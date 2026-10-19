//! The binary over a Vue repository: a component imported by TypeScript and by another component.

use std::path::Path;
use std::process::Command;

/// The binary as `tests/common` spawns it, with the globs set through the environment.
fn zerograph(repo: &Path, globs: &str, args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_zerograph"))
        .env_remove("ZEROGRAPH_BENCH_REPO")
        .env_remove("ZEROGRAPH_EMBED_MODEL")
        .env("ZEROGRAPH_CODE_GLOBS", globs)
        .arg("--no-dense")
        .arg("--repo")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

fn write(repo: &Path, rel: &str, text: &str) {
    let p = repo.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

const WITH_VUE: &str = "**/*.ts **/*.vue";

fn portal(repo: &Path, globs: &str) {
    write(repo, "tsconfig.json", "{ \"compilerOptions\": { \"baseUrl\": \".\", \"paths\": { \"@/*\": [\"src/*\"] } } }");
    write(repo, "src/components/Cart.vue", "<template>\n  <ul class=\"cart\"></ul>\n</template>\n");
    write(repo, "src/components/Checkout.vue", "<template>\n  <Cart />\n</template>\n\n<script lang=\"ts\">\nimport Cart from './Cart.vue';\nexport default class Checkout {\n  parts = [Cart];\n}\n</script>\n");
    write(repo, "src/routes.ts", "import Checkout from '@/components/Checkout.vue';\n\nexport const routes = [{ component: Checkout }];\n");
    let (ok, out, err) = zerograph(repo, globs, &["build"]);
    assert!(ok, "{out}{err}");
}

#[test]
fn impact_on_a_component_names_the_component_and_the_typescript_that_import_it() {
    let dir = tempfile::tempdir().unwrap();
    portal(dir.path(), WITH_VUE);
    let (ok, out, err) = zerograph(dir.path(), WITH_VUE, &["impact", "Cart"]);
    assert!(ok, "{err}");
    assert!(out.contains("src/components/Checkout.vue"), "{out}");
    let (ok, out, err) = zerograph(dir.path(), WITH_VUE, &["impact", "Checkout"]);
    assert!(ok, "{err}");
    assert!(out.contains("src/routes.ts"), "{out}");
}

#[test]
fn a_template_only_edit_re_reads_that_component_alone() {
    let dir = tempfile::tempdir().unwrap();
    portal(dir.path(), WITH_VUE);
    write(dir.path(), "src/components/Cart.vue", "<template>\n  <ol class=\"cart\"></ol>\n</template>\n");
    let (ok, out, err) = zerograph(dir.path(), WITH_VUE, &["update"]);
    assert!(ok, "{err}");
    assert!(out.starts_with("changed 1 removed 0"), "{out}");
}

#[test]
fn impact_on_a_kebab_case_component_names_the_file_importing_it_under_another_name() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    // The Vue style guide allows kebab-case file names, and every SFC is a default export.
    write(r, "src/components/order-list.vue", "<template>\n  <ul></ul>\n</template>\n");
    write(r, "src/routes.ts", "import OrderList from './components/order-list.vue';\n\nexport const routes = [{ component: OrderList }];\n");
    let (ok, out, err) = zerograph(r, WITH_VUE, &["build"]);
    assert!(ok, "{out}{err}");
    let (ok, out, err) = zerograph(r, WITH_VUE, &["impact", "sym:src/components/order-list.vue::order-list"]);
    assert!(ok, "{err}");
    assert!(out.contains("importers (1): src/routes.ts"), "a default import binds any name: {out}");
}

#[test]
fn a_vue_file_outside_the_globs_is_not_read() {
    let dir = tempfile::tempdir().unwrap();
    portal(dir.path(), "**/*.ts");
    let (ok, _, _) = zerograph(dir.path(), "**/*.ts", &["impact", "Cart"]);
    assert!(!ok, "no node is Cart when .vue is not globbed");
}
