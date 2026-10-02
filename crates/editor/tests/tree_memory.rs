//! Tree memory is returned when the size gate drops a tree, measured by
//! tree-sitter's own live bytes (`syntax::alloc::live_bytes`), so the check
//! does not depend on RSS or on the binary's global allocator. This file
//! holds one test so no other test allocates trees concurrently.

use eludite_editor::syntax::{Highlighter, LanguageRegistry, alloc::live_bytes};
use eludite_editor::text::{Buffer, BufferId, ReplicaId};

#[test]
fn dropping_the_tree_returns_its_memory() {
    let registry = LanguageRegistry::with_builtins();
    let mut source = String::new();
    for i in 0..5_000 {
        source.push_str(&format!(
            "fn f{i}(x: u32) -> u32 {{ let y = x * {i}; if y > 3 {{ y }} else {{ x }} }}\n"
        ));
    }
    let mut buffer = Buffer::new(ReplicaId::LOCAL, BufferId::new(1).unwrap(), source);
    let highlight_all = |h: &mut Highlighter, b: &Buffer| loop {
        if h.step(b.snapshot(), 0..0).unwrap().complete {
            break;
        }
    };
    let base = live_bytes();

    let mut kept = Highlighter::new(registry.by_id("rust").unwrap());
    kept.set_retain_limit(usize::MAX);
    highlight_all(&mut kept, &buffer);
    let tree = live_bytes() - base;
    assert!(
        tree > 10 * buffer.len(),
        "a tree takes far more than the text: {tree} bytes for {}",
        buffer.len()
    );
    drop(kept);

    let mut gated = Highlighter::new(registry.by_id("rust").unwrap());
    gated.set_retain_limit(0);
    highlight_all(&mut gated, &buffer);
    assert!(!gated.has_tree());
    let left = live_bytes().saturating_sub(base);
    assert!(left < tree / 20, "{left} bytes left of a {tree}-byte tree");

    buffer.edit([(0..0, "// edit\n")]);
    highlight_all(&mut gated, &buffer);
    assert!(!gated.has_tree());
    assert!(live_bytes().saturating_sub(base) < tree / 20);
}
