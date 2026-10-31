// The versions of the layout of gem's state in its database, each made by a migration from the one
// before. The server migrates the file when it opens it; the `gem` MCP server, reading it from
// another process, reads only the layout this build knows.
//
// A migration never changes once it has shipped; a change to the layout is a new one, with the
// next version.

use gem_db::{MapMut, Migration, Migrations};

pub fn migrations() -> Migrations {
    let migrations = [Migration::new(1, "index merges and rebases by thread", |moment: &mut MapMut| {
        gem_merges::rebuild_index(moment)?;
        gem_rebases::rebuild_index(moment).map(drop)
    })];
    Migrations::new(migrations).expect("migrations are in order")
}
