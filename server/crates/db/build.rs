use std::{env, fs, path::PathBuf};

// Embeds migrations/ as MIGRATIONS: each file's name without `.sql`, and its SQL, oldest first.
fn main() {
    println!("cargo:rerun-if-changed=migrations");
    let dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("migrations");
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "sql"))
        .collect();
    files.sort();
    let entries: String = (files.iter())
        .map(|path| {
            let name = path.file_stem().unwrap().to_str().unwrap();
            format!("({name:?}, include_str!({:?})),\n", path.to_str().unwrap())
        })
        .collect();
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("migrations.rs");
    fs::write(out, format!("const MIGRATIONS: &[(&str, &str)] = &[\n{entries}];\n")).unwrap();
}
