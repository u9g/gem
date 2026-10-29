// Runs the JavaScript agents pass gem's MCP servers, as Cloudflare's and Datadog's code mode do:
// an agent writes one script that reads, filters and joins what it needs, and only what the script
// returns reaches its context, instead of every tool call's whole answer.
//
// Scripts run in QuickJS, with no file system, network, clock beyond `Date`, or modules: only the
// `gem` object the server passes and `console`, whose lines come back with the result.

use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

use rquickjs::{Context, Function, Runtime};
use serde_json::{Value, json};

const TIMEOUT: Duration = match cfg!(test) {
    true => Duration::from_secs(1),
    false => Duration::from_secs(30),
};
const MEMORY_LIMIT: usize = 64 << 20;
// The most of a result an agent reads, in bytes, so a script that returns everything cannot fill
// its context.
const OUTPUT_LIMIT: usize = 100_000;

// Builds `gem` from `__call`, which the script cannot reach, then runs the script: an async arrow
// function, or failing that the body of one. `__settle` takes what it returns, or why it threw.
const PRELUDE: &str = r#"
(() => {
  const call = globalThis.__call, log = globalThis.__log, settle = globalThis.__settle;
  const code = globalThis.__code, ops = JSON.parse(globalThis.__ops);
  for (const name of ["__call", "__log", "__settle", "__code", "__ops"]) delete globalThis[name];
  const text = (value) => typeof value === "string" ? value : JSON.stringify(value, null, 2);
  const line = (...values) => log(values.map(text).join(" "));
  globalThis.console = Object.freeze({ log: line, info: line, warn: line, error: line, debug: line });
  const gem = {};
  for (const op of ops) {
    gem[op] = (...args) => {
      const answer = JSON.parse(call(op, JSON.stringify(args)));
      if ("error" in answer) throw new Error(answer.error);
      return answer.ok;
    };
  }
  globalThis.gem = Object.freeze(gem);
  // The script's own frames: this prelude's are `eval_script`'s.
  const failure = (error) => {
    if (!(error instanceof Error)) return text(error);
    const frames = (error.stack ?? "").split("\n").filter((frame) => frame.includes("<input>"));
    return [String(error), ...frames].join("\n");
  };
  let script;
  try {
    script = (0, eval)("(" + code + "\n)");
  } catch (error) {
    if (!(error instanceof SyntaxError)) return settle(false, failure(error));
    try {
      script = new (async () => {}).constructor(code);
    } catch (body) {
      return settle(false, `As a function: ${error}\nAs a function's body: ${body}`);
    }
  }
  (async () => (typeof script === "function" ? await script() : await script))().then(
    (value) => settle(true, value === undefined ? "undefined" : text(value)),
    (error) => settle(false, failure(error)),
  );
})();
"#;

// Runs `code` with `gem.<op>(...args)` for each of `ops`, answered by `call(op, args)`. Returns
// what the script logged and returned, or why it failed, for the agent to read.
pub fn run(
    code: &str,
    ops: &[&str],
    call: impl Fn(&str, Vec<Value>) -> Result<Value, String> + 'static,
) -> Result<String, String> {
    let runtime = Runtime::new().map_err(|e| e.to_string())?;
    runtime.set_memory_limit(MEMORY_LIMIT);
    runtime.set_max_stack_size(1 << 20);
    let deadline = Instant::now() + TIMEOUT;
    runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() > deadline)));
    let context = Context::full(&runtime).map_err(|e| e.to_string())?;

    let logs = Rc::new(RefCell::new(String::new()));
    let settled: Rc<RefCell<Option<Result<String, String>>>> = Rc::default();
    let started = context.with(|ctx| -> rquickjs::Result<()> {
        let globals = ctx.globals();
        globals.set("__code", code)?;
        globals.set("__ops", serde_json::to_string(ops).unwrap())?;
        let call = Function::new(ctx.clone(), move |op: String, args: String| {
            let args = serde_json::from_str(&args).unwrap_or_default();
            match call(&op, args) {
                Ok(value) => json!({ "ok": value }),
                Err(error) => json!({ "error": error }),
            }
            .to_string()
        })?;
        globals.set("__call", call)?;
        let log = Rc::clone(&logs);
        globals.set(
            "__log",
            Function::new(ctx.clone(), move |line: String| {
                let mut logs = log.borrow_mut();
                logs.push_str(&line);
                logs.push('\n');
            })?,
        )?;
        let settle = Rc::clone(&settled);
        globals.set(
            "__settle",
            Function::new(ctx.clone(), move |ok: bool, text: String| {
                *settle.borrow_mut() = Some(if ok { Ok(text) } else { Err(text) });
            })?,
        )?;
        ctx.eval::<(), _>(PRELUDE)
    });
    // An interrupted script stops with an exception that no `catch` can stop.
    let result = match started {
        Err(error) => Err(context.with(|ctx| describe(&ctx, error))),
        Ok(()) => {
            while settled.borrow().is_none() && runtime.is_job_pending() {
                if runtime.execute_pending_job().is_err() {
                    break;
                }
            }
            settled.take().unwrap_or(Err(
                "the script's promise never settled: await everything it starts".into(),
            ))
        }
    };
    let result = match Instant::now() > deadline {
        true => Err(format!("the script ran longer than {}s", TIMEOUT.as_secs())),
        false => result,
    };
    let logs = logs.take();
    let output = |text: String| match logs.is_empty() {
        true => truncate(text),
        false => truncate(format!("console:\n{logs}\nresult:\n{text}")),
    };
    result.map(output).map_err(output)
}

// An exception the script threw before it could settle, such as a syntax error.
fn describe(ctx: &rquickjs::Ctx, error: rquickjs::Error) -> String {
    match error {
        rquickjs::Error::Exception => {
            let exception = ctx.catch();
            match exception.as_exception() {
                Some(exception) => format!(
                    "{}\n{}",
                    exception.message().unwrap_or_default(),
                    exception.stack().unwrap_or_default()
                )
                .trim()
                .to_string(),
                None => format!("{exception:?}"),
            }
        }
        error => error.to_string(),
    }
}

fn truncate(mut text: String) -> String {
    if text.len() <= OUTPUT_LIMIT {
        return text;
    }
    let mut end = OUTPUT_LIMIT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let dropped = text.len() - end;
    text.truncate(end);
    text.push_str(&format!(
        "\n… {dropped} more bytes cut: return only what you need, such as a few fields or a filter's matches"
    ));
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn echo(code: &str) -> Result<String, String> {
        run(code, &["echo", "fail"], |op, args| match op {
            "echo" => Ok(json!(args)),
            _ => Err(format!("{op} failed")),
        })
    }

    #[test]
    fn an_async_function_returns_json() {
        assert_eq!(
            echo("async () => { const [x] = gem.echo(1, 2); return { x }; }").unwrap(),
            "{\n  \"x\": 1\n}"
        );
    }

    #[test]
    fn a_body_runs_as_an_async_function() {
        assert_eq!(
            echo("const a = await gem.echo('a'); return a[0];").unwrap(),
            "a"
        );
    }

    #[test]
    fn an_operation_error_throws_and_can_be_caught() {
        let error = echo("async () => gem.fail()").unwrap_err();
        assert!(error.starts_with("Error: fail failed"), "{error}");
        for code in [
            "async () => gem.fail()",
            "gem.fail()",
            "

return gem.fail();",
        ] {
            let error = echo(code).unwrap_err();
            assert!(
                error.contains("<input>:") && !error.contains("eval_script"),
                "{error}"
            );
        }
        assert_eq!(
            echo("async () => { try { gem.fail() } catch (e) { return e.message } }").unwrap(),
            "fail failed"
        );
    }

    #[test]
    fn logs_come_back_with_the_result() {
        assert_eq!(
            echo("async () => { console.log('n', 1, { a: 2 }); return 3; }").unwrap(),
            "console:\nn 1 {\n  \"a\": 2\n}\n\nresult:\n3"
        );
    }

    #[test]
    fn syntax_errors_and_runaway_scripts_fail() {
        assert!(echo("async () => {").unwrap_err().contains("SyntaxError"));
        let error = run("for (;;) {}", &[], |_, _| Ok(Value::Null)).unwrap_err();
        assert!(error.contains("ran longer"), "{error}");
    }

    #[test]
    fn the_bridge_is_out_of_reach() {
        assert_eq!(
            echo("async () => [typeof __call, typeof __settle, typeof fetch, typeof require]")
                .unwrap(),
            serde_json::to_string_pretty(&["undefined"; 4]).unwrap()
        );
    }

    #[test]
    fn long_results_are_cut() {
        let text = echo("async () => 'x'.repeat(200000)").unwrap();
        assert!(
            text.len() < 101_000 && text.contains("more bytes cut"),
            "{}",
            text.len()
        );
    }
}
