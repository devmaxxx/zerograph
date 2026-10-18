//! TypeScript, JavaScript and TSX extraction on inline sources and the staff fixture.

use super::*;

// ---- exports and declarations ----

#[test]
fn export_default_class_and_function_are_declared_as_exports() {
    let ex = extract("a.ts", "export default class Foo {\n  run() {}\n}\n");
    assert_eq!(declares(&ex, "Foo").as_deref(), Some("export"));
    assert!(ids(&ex).contains(&"sym:a.ts::Foo.run"));
    let ex = extract("b.ts", "export default function bar() { return 1; }\n");
    assert_eq!(declares(&ex, "bar").as_deref(), Some("export"));
}

#[test]
fn anonymous_default_exports_declare_nothing() {
    let ex = extract("a.ts", "export default class {}\nexport default () => 1;\n");
    assert!(ex.nodes.iter().all(|n| n.kind == NodeKind::File), "{:?}", ids(&ex));
}

#[test]
fn local_export_clause_marks_the_declarations_exported() {
    let ex = extract("a.ts", "const a = 1;\nfunction b() {}\nclass C {}\nexport { a, b as bee, C };\n");
    for s in ["a", "b", "C"] {
        assert_eq!(declares(&ex, s).as_deref(), Some("export"), "{s}");
    }
}

#[test]
fn several_declarators_in_one_statement_each_become_a_symbol() {
    let ex = extract("a.ts", "export const a = 1, b = 2;\nlet c = 3, d = 4;\n");
    for s in ["a", "b", "c", "d"] {
        assert!(ids(&ex).contains(&format!("sym:a.ts::{s}").as_str()), "{s}");
    }
    assert_eq!(declares(&ex, "b").as_deref(), Some("export"));
    assert_eq!(declares(&ex, "c").as_deref(), Some(""));
}

#[test]
fn destructured_declarations_name_each_binding() {
    let ex = extract("a.ts", "export const { x, y: why } = obj;\nconst [first, , third] = arr;\n");
    for s in ["x", "why", "first", "third"] {
        assert!(ids(&ex).contains(&format!("sym:a.ts::{s}").as_str()), "{s} in {:?}", ids(&ex));
    }
    assert!(!ids(&ex).iter().any(|i| i.contains('{') || i.contains('[')), "{:?}", ids(&ex));
}

#[test]
fn namespaces_enums_and_ambient_declarations_are_symbols() {
    let src = "export namespace Ns { export const inner = 1; }\nexport enum Color { Red }\nexport const enum Flag { On }\nexport declare const VERSION: string;\ndeclare function ambient(): void;\nexport declare class Amb {}\n";
    let ex = extract("a.ts", src);
    for s in ["Ns", "Color", "Flag", "VERSION", "ambient", "Amb"] {
        assert!(ids(&ex).contains(&format!("sym:a.ts::{s}").as_str()), "{s} in {:?}", ids(&ex));
    }
    assert_eq!(declares(&ex, "VERSION").as_deref(), Some("export"));
    assert_eq!(declares(&ex, "ambient").as_deref(), Some(""));
}

#[test]
fn function_overloads_collapse_into_one_symbol() {
    let ex = extract("a.ts", "export function f(a: string): string;\nexport function f(a: number): number;\nexport function f(a: any) { return a; }\n");
    assert_eq!(ex.nodes.iter().filter(|n| n.id == "sym:a.ts::f").count(), 1);
    assert_eq!(edges(&ex, EdgeKind::Declares), vec![("file:a.ts", "sym:a.ts::f", "export")]);
}

#[test]
fn type_only_and_satisfies_declarations_are_symbols() {
    let src = "export type Id = string;\nexport interface Shape { a: number }\nexport const cfg = { a: 1 } satisfies Shape;\nexport abstract class Base { abstract go(): void; }\n";
    let ex = extract("a.ts", src);
    for s in ["Id", "Shape", "cfg", "Base", "Base.go"] {
        assert!(ids(&ex).contains(&format!("sym:a.ts::{s}").as_str()), "{s} in {:?}", ids(&ex));
    }
}

#[test]
fn a_symbol_body_is_its_signature_line_except_for_functions() {
    let ex = extract("a.ts", "export class Big {\n  a = 1;\n  b = 2;\n}\nexport function f(\n  a: number,\n) {\n  return a;\n}\nexport const arrow = (x: number) =>\n  x + 1;\n");
    let body = |id: &str| ex.nodes.iter().find(|n| n.id == id).unwrap().body.clone();
    assert_eq!(body("sym:a.ts::Big"), "export class Big {");
    assert_eq!(body("sym:a.ts::f"), "export function f( a: number, ) { return a; }");
    assert_eq!(body("sym:a.ts::arrow"), "export const arrow = (x: number) =>");
}

#[test]
fn a_doc_comment_becomes_the_symbol_body_ahead_of_its_signature() {
    let ex = extract("a.ts", "/**\n * Revokes every session of the user: «выйти со всех устройств».\n * @see FR-SEC-23\n */\nexport function revokeAll(userId: string) {\n  return 1;\n}\n// One-line note.\nconst k = 1;\n");
    let body = |id: &str| ex.nodes.iter().find(|n| n.id == id).unwrap().body.clone();
    assert_eq!(body("sym:a.ts::revokeAll"), "Revokes every session of the user: «выйти со всех устройств».\n@see FR-SEC-23\nexport function revokeAll(userId: string) { return 1; }");
    assert_eq!(body("sym:a.ts::k"), "One-line note.\nconst k = 1;");
}

#[test]
fn a_comment_a_blank_line_above_is_a_section_heading_not_a_doc_comment() {
    let ex = extract("a.ts", "// ---- helpers ----\n\nexport const a = 1;\n/** two */\n/** parts */\nexport const b = 2;\n");
    let body = |id: &str| ex.nodes.iter().find(|n| n.id == id).unwrap().body.clone();
    assert_eq!(body("sym:a.ts::a"), "export const a = 1;");
    assert_eq!(body("sym:a.ts::b"), "two\nparts\nexport const b = 2;", "contiguous comments join, in order");
}

#[test]
fn a_class_member_takes_its_own_doc_comment() {
    let ex = extract("a.ts", "export class Svc {\n  /** Ends every session at once. */\n  revoke() {}\n\n  // plain\n  other() {}\n}\n");
    let body = |id: &str| ex.nodes.iter().find(|n| n.id == id).unwrap().body.clone();
    assert_eq!(body("sym:a.ts::Svc.revoke"), "Ends every session at once.\nrevoke() {}");
    assert_eq!(body("sym:a.ts::Svc.other"), "plain\nother() {}");
}

#[test]
fn the_file_node_carries_the_head_comment_and_nothing_else() {
    let ex = extract("a.ts", "/**\n * The auth surface's decisions.\n *\n * Two contours over one row.\n */\n// and a line\nimport x from 'y';\n/** not the head: below the first statement */\nexport const a = 1;\n");
    let file = ex.nodes.iter().find(|n| n.id == "file:a.ts").unwrap();
    assert_eq!(file.body, "The auth surface's decisions.\nTwo contours over one row.\nand a line");
    let bare = extract("b.ts", "import x from 'y';\nexport const a = 1;\n");
    assert_eq!(bare.nodes.iter().find(|n| n.id == "file:b.ts").unwrap().body, "");
}

#[test]
fn a_doc_comment_is_capped_so_the_signature_terms_survive() {
    let essay = format!("/**\n * {}\n */\nexport const a = 1;\n", "слово ".repeat(400));
    let ex = extract("a.ts", &essay);
    let body = ex.nodes.iter().find(|n| n.id == "sym:a.ts::a").unwrap().body.clone();
    assert!(body.ends_with("export const a = 1;"), "{body}");
    assert!(body.chars().count() < 700, "{}", body.chars().count());
}

/// A decorator above `export` belongs to the export statement, so the class starts at its own word;
/// one above a plain class is the class node's first child, so the class starts with it, as a
/// decorated field does.
#[test]
fn a_class_starts_where_its_node_does_its_decorator_included_unless_export_stands_between() {
    let ex = extract("a.ts", "\n@Injectable()\nexport class Svc {}\n\n@Injectable()\nclass Local {}\n");
    let line = |id: &str| ex.nodes.iter().find(|n| n.id == id).unwrap().line;
    assert_eq!(line("sym:a.ts::Svc"), 3);
    assert_eq!(line("sym:a.ts::Local"), 5);
}

// ---- class members ----

#[test]
fn every_member_kind_is_a_symbol_named_after_its_class() {
    let src = "export class A {\n  static count = 0;\n  private secret = 1;\n  readonly #hidden = 2;\n  constructor(private readonly dep: Dep) {}\n  get value() { return 1; }\n  set value(v: number) {}\n  async run() {}\n  static make() { return new A(); }\n  ['computed']() {}\n  'quoted-name'() {}\n  [Symbol.iterator]() {}\n  declare field: string;\n  optional?: number;\n}\n";
    let ex = extract("a.ts", src);
    let got = ids(&ex);
    for s in ["A.count", "A.secret", "A.#hidden", "A.constructor", "A.value", "A.run", "A.make", "A.computed", "A.quoted-name", "A.field", "A.optional"] {
        assert!(got.contains(&format!("sym:a.ts::{s}").as_str()), "{s} in {got:?}");
    }
    assert!(!got.iter().any(|i| i.contains('\'') || i.contains("[Symbol")), "{got:?}");
    assert_eq!(ex.nodes.iter().filter(|n| n.id == "sym:a.ts::A.value").count(), 1, "getter and setter share one symbol");
}

#[test]
fn class_expressions_and_nested_classes_are_not_scanned_for_members() {
    let ex = extract("a.ts", "export const A = class { run() {} };\nfunction f() { class Inner { go() {} } }\n");
    assert!(ids(&ex).contains(&"sym:a.ts::A"));
    assert!(!ids(&ex).iter().any(|i| i.contains("run") || i.contains("Inner")), "{:?}", ids(&ex));
}

#[test]
fn implements_is_not_extends() {
    let ex = extract("a.ts", "interface I {}\nexport class A implements I {}\nexport class B extends Object implements I {}\n");
    assert_eq!(edges(&ex, EdgeKind::Extends), vec![("sym:a.ts::B", "sym:a.ts::Object", "")]);
}

#[test]
fn implements_names_each_interface_through_its_import_without_type_arguments() {
    let repo = Repo::new(&[("port.ts", "export interface Port {}\nexport interface Sink<T> {}\n")]);
    let ex = repo.extract("a.ts", "import { Port, Sink } from './port';\ninterface Local {}\nexport class A extends Object implements Port, Sink<string>, Local {}\n");
    assert_eq!(edges(&ex, EdgeKind::Implements), vec![
        ("sym:a.ts::A", "sym:a.ts::Local", ""),
        ("sym:a.ts::A", "sym:port.ts::Port", ""),
        ("sym:a.ts::A", "sym:port.ts::Sink", ""),
    ]);
}

#[test]
fn the_methods_and_function_properties_of_an_interface_are_its_members() {
    let src = "export interface Port {\n  /** Suggests the next visit. */\n  suggest(id: string): Promise<void>;\n  maybe?(): void;\n  notify: (x: number) => void;\n  [key: string]: unknown;\n}\n";
    let ex = extract("a.ts", src);
    for s in ["Port.suggest", "Port.maybe", "Port.notify"] {
        let id = format!("sym:a.ts::{s}");
        assert!(ids(&ex).contains(&id.as_str()), "{s} in {:?}", ids(&ex));
        assert!(edges(&ex, EdgeKind::Declares).contains(&("sym:a.ts::Port", id.as_str(), "")), "{s} is declared by its interface");
    }
    let m = node(&ex, "sym:a.ts::Port.suggest");
    assert_eq!((m.line, m.body.as_str()), (3, "Suggests the next visit.\nsuggest(id: string): Promise<void>"));
}

#[test]
fn the_data_properties_of_an_interface_are_its_members_and_its_index_signature_is_not() {
    let src = "export interface Range {\n  readonly from: string;\n  to?: string;\n  'quoted-key': number;\n  [key: string]: unknown;\n}\n";
    let ex = extract("a.ts", src);
    for (s, line) in [("Range.from", 2), ("Range.to", 3), ("Range.quoted-key", 4)] {
        let id = format!("sym:a.ts::{s}");
        assert!(edges(&ex, EdgeKind::Declares).contains(&("sym:a.ts::Range", id.as_str(), "")), "{s} is declared by its interface: {:?}", ids(&ex));
        assert_eq!(node(&ex, &id).line, line, "{s}");
    }
    assert!(!ids(&ex).iter().any(|i| i.contains("key") && !i.contains("quoted")), "an index signature names no member: {:?}", ids(&ex));
}

#[test]
fn a_components_props_interface_declares_each_prop() {
    let src = "interface GridProps {\n  readonly range: Range;\n  readonly loading?: boolean;\n}\nexport function Grid({ range }: GridProps) {\n  return <div>{range.from}</div>;\n}\n";
    let ex = extract("a.tsx", src);
    for s in ["GridProps.range", "GridProps.loading"] {
        let id = format!("sym:a.tsx::{s}");
        assert!(edges(&ex, EdgeKind::Declares).contains(&("sym:a.tsx::GridProps", id.as_str(), "")), "{s} in {:?}", ids(&ex));
    }
}

#[test]
fn the_data_properties_of_an_ambient_interface_are_its_members() {
    let ex = extract("sim.d.ts", "export declare interface Batch {\n  readonly ops_sent: number;\n}\ndeclare interface Local {\n  id: string;\n}\n");
    for s in ["Batch.ops_sent", "Local.id"] {
        assert!(ids(&ex).contains(&format!("sym:sim.d.ts::{s}").as_str()), "{s} in {:?}", ids(&ex));
    }
}

#[test]
fn extends_drops_type_arguments_and_follows_the_import() {
    let repo = Repo::new(&[("base.ts", "export class Base<T> {}\n")]);
    let ex = repo.extract("a.ts", "import { Base } from './base';\nexport class A extends Base<string> {}\nclass L {}\nclass M extends L {}\n");
    let ext = edges(&ex, EdgeKind::Extends);
    assert!(ext.contains(&("sym:a.ts::A", "sym:base.ts::Base", "")), "{ext:?}");
    assert!(ext.contains(&("sym:a.ts::M", "sym:a.ts::L", "")), "{ext:?}");
}

#[test]
fn extends_of_a_call_or_member_expression_keeps_the_expression_text() {
    let ex = extract("a.ts", "class A extends mixin(Base) {}\nclass B extends Outer.Inner {}\n");
    let ext = edges(&ex, EdgeKind::Extends);
    assert!(ext.contains(&("sym:a.ts::A", "sym:a.ts::mixin(Base)", "")), "{ext:?}");
    assert!(ext.contains(&("sym:a.ts::B", "sym:a.ts::Outer.Inner", "")), "{ext:?}");
}

// ---- object-literal members ----

#[test]
fn the_methods_and_function_properties_of_a_top_level_literal_are_its_members() {
    let src = "export const repo = {\n  find() {\n    return 1;\n  },\n  save: async (x: number) => x,\n  drop: function () {},\n  gen: function* () {},\n  limit: 10,\n  get size() { return 1; },\n  'quoted-name'() {},\n  shorthand,\n  ...rest,\n} as const;\n";
    let ex = extract("a.ts", src);
    let got = ids(&ex);
    for s in ["repo.find", "repo.save", "repo.drop", "repo.gen", "repo.size", "repo.quoted-name"] {
        assert!(got.contains(&format!("sym:a.ts::{s}").as_str()), "{s} in {got:?}");
    }
    for s in ["repo.limit", "repo.shorthand", "repo.rest"] {
        assert!(!got.contains(&format!("sym:a.ts::{s}").as_str()), "a value that is not a function is no member: {s}");
    }
    assert!(edges(&ex, EdgeKind::Declares).contains(&("sym:a.ts::repo", "sym:a.ts::repo.find", "")));
    assert_eq!(declares(&ex, "repo").as_deref(), Some("export"));
    let find = node(&ex, "sym:a.ts::repo.find");
    assert_eq!((find.label.as_str(), find.line, find.end), ("repo.find", 2, 4));
    assert_eq!(node(&ex, "sym:a.ts::repo.save").line, 5);
}

/// One wrapper is looked through; two nested are not, since each costs the query's compile and no
/// input writes them.
#[test]
fn a_literal_behind_satisfies_or_parentheses_still_names_its_members() {
    for src in ["const routes = { home() {} } satisfies Routes;\n", "const routes = ({ home() {} });\n", "const routes = { home() {} } as Routes;\n"] {
        let ex = extract("a.ts", src);
        assert!(ids(&ex).contains(&"sym:a.ts::routes.home"), "{src}: {:?}", ids(&ex));
    }
    let ex = extract("a.ts", "const routes = ({ home() {} }) satisfies Routes;\n");
    assert!(!ids(&ex).contains(&"sym:a.ts::routes.home"), "{:?}", ids(&ex));
}

#[test]
fn a_nested_literal_and_a_literal_inside_a_function_are_not_scanned_for_members() {
    let ex = extract("a.ts", "export const a = { inner: { deep() {} } };\nfunction f() { const b = { m() {} }; return b; }\n");
    let got = ids(&ex);
    assert!(!got.iter().any(|i| i.contains("deep") || i.contains("inner") || i.contains("b.m")), "{got:?}");
}

#[test]
fn a_call_and_an_id_inside_a_literal_method_belong_to_that_method() {
    let repo = Repo::new(&[("lib.ts", "export function lock() {}\n")]);
    let ex = repo.extract("r.ts", "import { lock } from './lib';\nexport const repo = {\n  guard() { lock(); return 'FR-PAY-01'; },\n  other: () => 'INV-11',\n};\n");
    assert_eq!(calls(&ex), vec![("sym:r.ts::repo.guard", "sym:lib.ts::lock")]);
    let refs = edges(&ex, EdgeKind::References);
    assert!(refs.contains(&("sym:r.ts::repo.guard", "FR-PAY-01", "string")), "{refs:?}");
    assert!(refs.contains(&("sym:r.ts::repo.other", "INV-11", "string")), "{refs:?}");
}

#[test]
fn this_in_a_literal_method_targets_the_sibling_member() {
    let ex = extract("r.ts", "export const repo = {\n  a() { this.b(); },\n  b() {},\n  c: () => this.b(),\n};\n");
    assert_eq!(calls(&ex), vec![("sym:r.ts::repo.a", "sym:r.ts::repo.b")], "an arrow's `this` is not the literal");
}

#[test]
fn a_call_on_an_imported_literal_lands_on_the_member_its_file_declares() {
    let literal = "export const repo = { find() {} };\n";
    let repo = Repo::new(&[("r.ts", literal)]);
    let ex = repo.extract("a.ts", "import { repo } from './r';\nexport function run() { repo.find(); }\n");
    assert_eq!(calls(&ex), vec![("sym:a.ts::run", "sym:r.ts::repo.find")]);
    assert!(ids(&repo.extract("r.ts", literal)).contains(&"sym:r.ts::repo.find"));
}

#[test]
fn a_literal_returned_from_a_class_method_does_not_rename_the_method() {
    let ex = extract("a.ts", "class A {\n  run() { return { foo() { return 'FR-PAY-01'; } }; }\n}\n");
    let refs = edges(&ex, EdgeKind::References);
    assert_eq!(refs, vec![("sym:a.ts::A.run", "FR-PAY-01", "string")]);
}

// ---- decorators ----

#[test]
fn decorator_arguments_of_every_shape() {
    let src = "@Controller({ path: 'x' })\n@Ns.deco('ns')\n@plain\nexport class A {\n  @Tpl(`tpl`)\n  @Roles('first', 'second')\n  @Sym(Actions.X)\n  @Mixed(42, 'late')\n  run() {}\n}\n";
    let ex = extract("a.ts", src);
    let d = edges(&ex, EdgeKind::DecoratedBy);
    for want in [
        ("sym:a.ts::A", "deco:Controller", ""),
        ("sym:a.ts::A", "deco:Ns.deco", "ns"),
        ("sym:a.ts::A", "deco:plain", ""),
        ("sym:a.ts::A.run", "deco:Tpl", "tpl"),
        ("sym:a.ts::A.run", "deco:Roles", "first"),
        ("sym:a.ts::A.run", "deco:Sym", ""),
        ("sym:a.ts::A.run", "deco:Mixed", "late"),
    ] {
        assert!(d.contains(&want), "{want:?} in {d:?}");
    }
}

#[test]
fn a_decorator_queued_before_a_non_member_does_not_leak_onto_the_next_method() {
    let src = "class A {\n  @Leak()\n  [key: string]: unknown;\n  run() {}\n  @Own()\n  go() {}\n}\n";
    let ex = extract("a.ts", src);
    let d = edges(&ex, EdgeKind::DecoratedBy);
    assert!(!d.iter().any(|e| e.0 == "sym:a.ts::A.run"), "{d:?}");
    assert!(d.contains(&("sym:a.ts::A.go", "deco:Own", "")), "{d:?}");
}

#[test]
fn decorator_nodes_are_shared_across_files_by_name() {
    let ex = extract("a.ts", "@Injectable()\nexport class A {}\n@Injectable()\nexport class B {}\n");
    assert_eq!(ex.nodes.iter().filter(|n| n.id == "deco:Injectable").count(), 1);
    assert!(ex.nodes.iter().filter(|n| n.id == "deco:Injectable").all(|n| n.kind == NodeKind::Symbol && n.label == "Injectable"));
}

// ---- imports and re-exports ----

#[test]
fn import_forms_name_what_they_bring_in() {
    let repo = Repo::new(&[("lib.ts", "export {};\n"), ("side.ts", "export {};\n"), ("legacy.ts", "export {};\n")]);
    let src = "import def, { a, b as bee } from './lib';\nimport * as ns from './lib';\nimport type { T } from './lib';\nimport { type U, v } from './lib';\nimport './side';\nimport legacy = require('./legacy');\n";
    let ex = repo.extract("a.ts", src);
    let imp = edges(&ex, EdgeKind::Imports);
    let ctx: Vec<&str> = imp.iter().filter(|e| e.1 == "file:lib.ts").map(|e| e.2).collect();
    assert_eq!(ctx, vec!["*", "T", "U,v", "def,a,b"], "{imp:?}");
    assert!(imp.contains(&("file:a.ts", "file:side.ts", "")), "{imp:?}");
    assert!(imp.contains(&("file:a.ts", "file:legacy.ts", "legacy")), "{imp:?}");
}

#[test]
fn re_export_forms() {
    let repo = Repo::new(&[("lib.ts", "export {};\n")]);
    let src = "export * from './lib';\nexport * as ns from './lib';\nexport { a, b as bee } from './lib';\nexport type { T } from './lib';\n";
    let ex = repo.extract("a.ts", src);
    let mut re: Vec<&str> = edges(&ex, EdgeKind::ReExports).iter().map(|e| e.2).collect();
    re.sort();
    assert_eq!(re, vec!["*", "T", "a,b as bee", "ns"]);
}

#[test]
fn a_local_renamed_export_re_exports_the_file_to_itself_under_the_alias() {
    let ex = extract("a.ts", "function contrast() {}\nfunction keep() {}\nexport { contrast as contrastRatio, keep };\n");
    assert_eq!(edges(&ex, EdgeKind::ReExports), vec![("file:a.ts", "file:a.ts", "contrast as contrastRatio")]);
    assert_eq!(declares(&ex, "contrast").as_deref(), Some("export"));
    assert!(edges(&extract("b.ts", "const a = 1;\nexport { a };\n"), EdgeKind::ReExports).is_empty(), "no rename, no edge");
}

#[test]
fn unresolvable_imports_leave_no_edge() {
    let ex = extract("a.ts", "import x from 'react';\nimport y from './missing';\nimport z from 'node:fs';\nexport * from '@scope/pkg';\n");
    assert!(edges(&ex, EdgeKind::Imports).is_empty());
    assert!(edges(&ex, EdgeKind::ReExports).is_empty());
}

#[test]
fn dynamic_imports_are_edges_from_the_importing_symbol() {
    let repo = Repo::new(&[("lazy.ts", "export {};\n")]);
    let ex = repo.extract("a.ts", "export async function load() { return import('./lazy'); }\nconst m = require('./lazy');\n");
    let imp = edges(&ex, EdgeKind::Imports);
    assert!(imp.contains(&("sym:a.ts::load", "file:lazy.ts", "")), "{imp:?}");
    assert!(imp.contains(&("sym:a.ts::m", "file:lazy.ts", "")), "{imp:?}");
}

// ---- parsing ----

#[test]
fn tsx_parses_jsx_and_ts_does_not_need_to() {
    let ex = extract("App.tsx", "export function App() { return <div className=\"x\">hi</div>; }\nexport const Item = () => <li />;\n");
    assert!(ids(&ex).contains(&"sym:App.tsx::App"));
    assert!(ids(&ex).contains(&"sym:App.tsx::Item"));
    let ex = extract("g.ts", "export const id = <T,>(x: T): T => x;\nexport function g<T extends object>(x: T) { return x; }\n");
    assert!(ids(&ex).contains(&"sym:g.ts::id"));
    assert!(ids(&ex).contains(&"sym:g.ts::g"));
}

#[test]
fn a_syntax_error_keeps_the_declarations_around_it() {
    let ex = extract("a.ts", "export const before = 1;\nexport function broken( {\nexport const after = 2;\nexport class After {}\n");
    let got = ids(&ex);
    assert!(got.contains(&"sym:a.ts::before"), "{got:?}");
    assert!(got.contains(&"sym:a.ts::after") || got.contains(&"sym:a.ts::After"), "{got:?}");
}

#[test]
fn an_empty_or_comment_only_file_is_just_a_file_node() {
    for src in ["", "// nothing\n", "/* FR-PAY-03 */\n"] {
        let ex = extract("a.ts", src);
        assert_eq!(ex.nodes.len(), 1, "{src:?}");
        assert_eq!(ex.nodes[0].id, "file:a.ts");
        assert_eq!(ex.nodes[0].kind, NodeKind::File);
    }
}

#[test]
fn cyrillic_identifiers_and_strings_do_not_break_offsets() {
    let src = "export const цена = 'Сумма FR-PAY-22';\n// после INV-03\nexport function после() { return `см. FR-SEC-21 и ещё`; }\n";
    let ex = extract("a.ts", src);
    assert!(ids(&ex).contains(&"sym:a.ts::цена"), "{:?}", ids(&ex));
    assert!(ids(&ex).contains(&"sym:a.ts::после"));
    let refs = edges(&ex, EdgeKind::References);
    assert!(refs.contains(&("sym:a.ts::после", "FR-SEC-21", "string")), "{refs:?}");
    assert!(refs.contains(&("file:a.ts", "INV-03", "comment")), "{refs:?}");
}

// ---- id references ----

#[test]
fn ids_in_an_arrow_const_attach_to_the_const() {
    let ex = extract("a.ts", "export const handler = () => {\n  // FR-PAY-03\n  return 'INV-11';\n};\nfunction outer() { const inner = () => 'FR-SEC-21'; return inner; }\n");
    let refs = edges(&ex, EdgeKind::References);
    assert!(refs.contains(&("sym:a.ts::handler", "FR-PAY-03", "comment")), "{refs:?}");
    assert!(refs.contains(&("sym:a.ts::handler", "INV-11", "string")), "{refs:?}");
    assert!(refs.contains(&("sym:a.ts::outer", "FR-SEC-21", "string")), "a nested const is not a symbol: {refs:?}");
}

#[test]
fn ids_in_a_nested_function_inside_a_method_attach_to_the_method() {
    let ex = extract("a.ts", "class A {\n  run() {\n    const f = () => { /* FR-PAY-03 */ };\n    function g() { return 'INV-11'; }\n  }\n}\n");
    let refs = edges(&ex, EdgeKind::References);
    assert!(refs.contains(&("sym:a.ts::A.run", "FR-PAY-03", "comment")), "{refs:?}");
    assert!(refs.contains(&("sym:a.ts::A.run", "INV-11", "string")), "{refs:?}");
}

#[test]
fn ranges_and_slash_lists_in_comments_expand() {
    let ex = extract("a.ts", "// covers FR-RPT-42…44 and INV-11/12\n");
    let mut targets: Vec<&str> = edges(&ex, EdgeKind::References).iter().map(|e| e.1).collect();
    targets.sort();
    assert_eq!(targets, vec!["FR-RPT-42", "FR-RPT-43", "FR-RPT-44", "INV-11", "INV-12"]);
}

#[test]
fn the_same_id_twice_in_one_owner_is_one_edge() {
    let ex = extract("a.ts", "// FR-PAY-03 and again FR-PAY-03\nexport function f() { return 'FR-PAY-03'; }\n");
    let refs = edges(&ex, EdgeKind::References);
    assert_eq!(refs.iter().filter(|e| e.0 == "file:a.ts" && e.1 == "FR-PAY-03").count(), 1, "{refs:?}");
    assert_eq!(refs.iter().filter(|e| e.0 == "sym:a.ts::f").count(), 1, "{refs:?}");
}

#[test]
fn ids_in_regex_literals_jsx_text_and_identifiers_are_not_references() {
    let ex = extract("a.tsx", "const re = /FR-PAY-03/;\nconst FR_PAY_03 = 1;\nexport const V = () => <p>FR-SEC-21</p>;\n");
    assert!(edges(&ex, EdgeKind::References).is_empty(), "{:?}", edges(&ex, EdgeKind::References));
}

#[test]
fn ids_in_a_decorator_argument_attach_to_the_decorated_member() {
    let ex = extract("a.ts", "class A {\n  @RequireAction('FR-VIS-35')\n  run() {}\n}\n");
    let refs = edges(&ex, EdgeKind::References);
    assert!(refs.contains(&("sym:a.ts::A.run", "FR-VIS-35", "string")), "{refs:?}");
}

#[test]
fn ids_in_test_titles_and_describe_blocks_attach_to_the_file() {
    let src = "describe('FR-VIS-35 templates', () => {\n  it('INV-11 holds', () => {});\n  test.each([['FR-PAY-03']])('%s', () => {});\n});\n";
    let ex = extract("t.spec.ts", src);
    let mut refs = edges(&ex, EdgeKind::References);
    refs.sort();
    assert_eq!(refs, vec![("file:t.spec.ts", "FR-PAY-03", "string"), ("file:t.spec.ts", "FR-VIS-35", "string"), ("file:t.spec.ts", "INV-11", "string")]);
}

#[test]
fn ids_in_interfaces_enums_and_types_attach_to_that_symbol() {
    let src = "export interface I {\n  /** FR-PAY-03 */\n  a: number;\n}\nenum E { A = 'INV-11' }\ntype T = { k: 'FR-SEC-21' };\nexport namespace N { export const q = 'FR-VIS-35'; }\n";
    let ex = extract("a.ts", src);
    let refs = edges(&ex, EdgeKind::References);
    assert!(refs.contains(&("sym:a.ts::I", "FR-PAY-03", "comment")), "{refs:?}");
    assert!(refs.contains(&("sym:a.ts::E", "INV-11", "string")), "{refs:?}");
    assert!(refs.contains(&("sym:a.ts::T", "FR-SEC-21", "string")), "{refs:?}");
    assert!(refs.contains(&("sym:a.ts::N", "FR-VIS-35", "string")), "{refs:?}");
}

#[test]
fn ids_in_a_class_property_initialiser_attach_to_the_property() {
    let ex = extract("a.ts", "class A {\n  handler = () => 'FR-PAY-03';\n  static {\n    console.log('INV-11');\n  }\n}\n");
    let refs = edges(&ex, EdgeKind::References);
    assert!(refs.contains(&("sym:a.ts::A.handler", "FR-PAY-03", "string")), "{refs:?}");
    assert!(refs.contains(&("sym:a.ts::A", "INV-11", "string")), "{refs:?}");
}

#[test]
fn ids_in_jsx_attributes_are_strings_of_the_component() {
    let ex = extract("a.tsx", "export const View = () => <Route path=\"x\" title=\"FR-PAY-03\" />;\n");
    let refs = edges(&ex, EdgeKind::References);
    assert_eq!(refs, vec![("sym:a.tsx::View", "FR-PAY-03", "string")]);
}

#[test]
fn hyphenless_labels_are_not_references_and_a_one_letter_family_is() {
    // A label with no hyphen before its digits is not an id in any corpus; `I-015` has the shape
    // an id has, and whether `I` is a family is the graph's question, not this scan's.
    let ex = extract("a.ts", "// step B1, table C11, size S3, item I-015\n");
    let refs = edges(&ex, EdgeKind::References);
    assert_eq!(refs, vec![("file:a.ts", "I-015", "comment")], "{refs:?}");
}


// ---- spans ----

fn node<'a>(ex: &'a Extraction, id: &str) -> &'a crate::model::Node {
    ex.nodes.iter().find(|n| n.id == id).unwrap_or_else(|| panic!("{id} not extracted"))
}

#[test]
fn a_function_declaration_spans_its_body() {
    let ex = extract("a.ts", "export function f() {\n  return 1;\n}\nconst x = 1;\n");
    let f = node(&ex, "sym:a.ts::f");
    assert_eq!((f.line, f.end), (1, 3));
    let x = node(&ex, "sym:a.ts::x");
    assert_eq!((x.line, x.end), (4, 4));
}

#[test]
fn a_class_and_each_member_carry_their_own_span() {
    let ex = extract("a.ts", "export class A {\n  one() {\n    return 1;\n  }\n\n  two() {}\n}\n");
    assert_eq!((node(&ex, "sym:a.ts::A").line, node(&ex, "sym:a.ts::A").end), (1, 7));
    assert_eq!((node(&ex, "sym:a.ts::A.one").line, node(&ex, "sym:a.ts::A.one").end), (2, 4));
    assert_eq!((node(&ex, "sym:a.ts::A.two").line, node(&ex, "sym:a.ts::A.two").end), (6, 6));
}

#[test]
fn a_decorated_export_class_still_starts_at_its_name_and_ends_at_its_brace() {
    let ex = extract("a.ts", "@Injectable()\nexport class S {\n  run() {}\n}\n");
    let s = node(&ex, "sym:a.ts::S");
    assert_eq!((s.line, s.end), (2, 4));
}

#[test]
fn a_decorator_node_has_no_span() {
    let ex = extract("a.ts", "@Injectable()\nexport class S {}\n");
    assert_eq!(node(&ex, "deco:Injectable").end, 0);
}

// ---- calls ----

fn calls(ex: &Extraction) -> Vec<(&str, &str)> {
    edges(ex, EdgeKind::Calls).into_iter().map(|(s, t, _)| (s, t)).collect()
}

#[test]
fn a_call_to_an_imported_function_targets_the_symbol_in_its_file() {
    let repo = Repo::new(&[("lib.ts", "export function helper() {}\n")]);
    let ex = repo.extract("a.ts", "import { helper } from './lib';\nexport function run() { helper(); }\n");
    assert_eq!(calls(&ex), vec![("sym:a.ts::run", "sym:lib.ts::helper")]);
}

#[test]
fn new_of_an_imported_class_is_a_call() {
    let repo = Repo::new(&[("foo.ts", "export class Foo {}\n")]);
    let ex = repo.extract("a.ts", "import { Foo } from './foo';\nexport const make = () => new Foo();\n");
    assert_eq!(calls(&ex), vec![("sym:a.ts::make", "sym:foo.ts::Foo")]);
}

#[test]
fn an_aliased_import_is_resolved_to_the_name_its_file_declares() {
    let repo = Repo::new(&[("lib.ts", "export function helper() {}\n")]);
    let ex = repo.extract("a.ts", "import { helper as h } from './lib';\nexport function run() { h(); }\n");
    assert_eq!(calls(&ex), vec![("sym:a.ts::run", "sym:lib.ts::helper")]);
}

#[test]
fn a_call_to_a_local_top_level_function_stays_in_the_file() {
    let ex = extract("a.ts", "function inner() {}\nexport function run() { inner(); }\n");
    assert_eq!(calls(&ex), vec![("sym:a.ts::run", "sym:a.ts::inner")]);
}

#[test]
fn a_call_to_a_name_the_file_cannot_prove_yields_no_edge() {
    let ex = extract("a.ts", "export function run(cb: () => void) { console.log(1); cb(); fetch('/'); }\n");
    assert!(calls(&ex).is_empty());
}

#[test]
fn this_method_targets_the_enclosing_class_member() {
    let ex = extract("a.ts", "export class A {\n  run() { this.other(); }\n  other() {}\n}\n");
    assert_eq!(calls(&ex), vec![("sym:a.ts::A.run", "sym:a.ts::A.other")]);
}

#[test]
fn a_static_call_on_an_imported_class_targets_its_member() {
    let repo = Repo::new(&[("util.ts", "export class Util { static go() {} }\n")]);
    let ex = repo.extract("a.ts", "import { Util } from './util';\nexport function run() { Util.go(); }\n");
    assert_eq!(calls(&ex), vec![("sym:a.ts::run", "sym:util.ts::Util.go")]);
}

#[test]
fn a_namespace_import_call_targets_the_bare_symbol() {
    let repo = Repo::new(&[("n.ts", "export function fn() {}\n")]);
    let ex = repo.extract("a.ts", "import * as ns from './n';\nexport function run() { ns.fn(); }\n");
    assert_eq!(calls(&ex), vec![("sym:a.ts::run", "sym:n.ts::fn")]);
}

#[test]
fn a_call_through_a_constructor_parameter_property_targets_the_injected_class_member() {
    let repo = Repo::new(&[("s.ts", "export class StaffService { create() {} }\n")]);
    let ex = repo.extract(
        "c.ts",
        "import { StaffService } from './s';\nexport class StaffController {\n  constructor(private readonly service: StaffService) {}\n  create(dto: unknown) { return this.service.create(dto); }\n}\n",
    );
    assert_eq!(calls(&ex), vec![("sym:c.ts::StaffController.create", "sym:s.ts::StaffService.create")]);
}

#[test]
fn a_call_through_a_typed_field_targets_the_field_type_member() {
    let repo = Repo::new(&[("r.ts", "export class Repo { find() {} }\n")]);
    let ex = repo.extract("c.ts", "import { Repo } from './r';\nexport class C {\n  private repo: Repo;\n  run() { this.repo.find(); }\n}\n");
    assert_eq!(calls(&ex), vec![("sym:c.ts::C.run", "sym:r.ts::Repo.find")]);
}

#[test]
fn a_function_passed_as_an_argument_is_called_by_the_caller() {
    let repo = Repo::new(&[("w.ts", "export function feedWire() {}\n")]);
    let ex = repo.extract(
        "center.service.ts",
        "import { feedWire } from './w';\nfunction isIndexed() { return true; }\n\
         export function list(rows: unknown[], limit: number) { return rows.filter(isIndexed).map(feedWire).slice(0, limit); }\n",
    );
    let mut got = calls(&ex);
    got.sort();
    assert_eq!(got, vec![
        ("sym:center.service.ts::list", "sym:center.service.ts::isIndexed"),
        ("sym:center.service.ts::list", "sym:w.ts::feedWire"),
    ], "a parameter passed along proves nothing and gives no edge");
}

#[test]
fn a_value_passed_as_an_argument_is_an_arg_edge_unless_the_owner_also_calls_it() {
    let repo = Repo::new(&[("t.ts", "export const TOKEN = 'T';\nexport function run() {}\n")]);
    let ex = repo.extract(
        "m.ts",
        "import { TOKEN, run } from './t';\n\
         export function a() { register(TOKEN); }\n\
         export function b() { go(run); run(); }\n",
    );
    let mut got = edges(&ex, EdgeKind::Calls);
    got.sort();
    assert_eq!(got, vec![
        ("sym:m.ts::a", "sym:t.ts::TOKEN", "arg"),
        ("sym:m.ts::b", "sym:t.ts::run", ""),
    ], "one edge per pair, and a real call wins over the same target passed");
}

#[test]
fn rendering_a_component_in_jsx_is_a_call_of_it() {
    let repo = Repo::new(&[
        ("m.tsx", "export function CommissionMatrix() { return null; }\n"),
        ("ui.tsx", "export function Card() { return null; }\nexport function Body() { return null; }\n"),
    ]);
    let ex = repo.extract(
        "screen.tsx",
        "import { CommissionMatrix } from './m';\nimport * as ui from './ui';\nconst div = () => 1;\n\
         export function Screen() {\n  return <div><ui.Card><CommissionMatrix rows={[]} /></ui.Card><ui.Body/></div>;\n}\n",
    );
    let mut got = calls(&ex);
    got.sort();
    assert_eq!(got, vec![
        ("sym:screen.tsx::Screen", "sym:m.tsx::CommissionMatrix"),
        ("sym:screen.tsx::Screen", "sym:ui.tsx::Body"),
        ("sym:screen.tsx::Screen", "sym:ui.tsx::Card"),
    ]);
}

#[test]
fn a_call_on_a_helpers_return_value_targets_the_returned_class_member() {
    let repo = Repo::new(&[("s.ts", "export class PoliciesService { cancellationPolicy() {} }\n")]);
    let annotated = repo.extract(
        "a.spec.ts",
        "import { PoliciesService } from './s';\nconst service = (): PoliciesService => make();\nit('x', async () => { await service().cancellationPolicy(); });\n",
    );
    assert!(calls(&annotated).contains(&("file:a.spec.ts", "sym:s.ts::PoliciesService.cancellationPolicy")));
    let built = repo.extract(
        "b.spec.ts",
        "import { PoliciesService } from './s';\nfunction service() { return x; }\nconst other = () => new PoliciesService();\nit('x', () => other().cancellationPolicy());\nit('y', () => service().cancellationPolicy());\n",
    );
    let got = calls(&built);
    assert!(got.contains(&("file:b.spec.ts", "sym:s.ts::PoliciesService.cancellationPolicy")));
    assert_eq!(got.iter().filter(|c| c.1.ends_with("cancellationPolicy")).count(), 1, "an unannotated block body proves nothing");
}

#[test]
fn a_generic_field_type_uses_its_head_name() {
    let repo = Repo::new(&[("r.ts", "export class Repository<T> { find() {} }\n")]);
    let ex = repo.extract("c.ts", "import { Repository } from './r';\nexport class C {\n  constructor(private readonly users: Repository<User>) {}\n  run() { this.users.find(); }\n}\n");
    assert_eq!(calls(&ex), vec![("sym:c.ts::C.run", "sym:r.ts::Repository.find")]);
}

#[test]
fn a_call_through_a_barrel_points_at_the_barrel_and_stays_dangling() {
    let repo = Repo::new(&[("lib/index.ts", "export * from './impl';\n"), ("lib/impl.ts", "export function helper() {}\n")]);
    let ex = repo.extract("a.ts", "import { helper } from './lib';\nexport function run() { helper(); }\n");
    assert_eq!(calls(&ex), vec![("sym:a.ts::run", "sym:lib/index.ts::helper")]);
}

#[test]
fn a_call_outside_any_symbol_is_owned_by_the_file() {
    let repo = Repo::new(&[("lib.ts", "export function boot() {}\n")]);
    let ex = repo.extract("a.ts", "import { boot } from './lib';\nboot();\n");
    assert_eq!(calls(&ex), vec![("file:a.ts", "sym:lib.ts::boot")]);
}

#[test]
fn a_recursive_call_is_not_an_edge_and_a_repeated_call_is_one_edge() {
    let repo = Repo::new(&[("lib.ts", "export function helper() {}\n")]);
    let ex = repo.extract("a.ts", "import { helper } from './lib';\nexport function run(n: number) { helper(); helper(); if (n) run(n - 1); }\n");
    assert_eq!(calls(&ex), vec![("sym:a.ts::run", "sym:lib.ts::helper")]);
}

#[test]
fn a_chained_call_and_super_yield_nothing() {
    let repo = Repo::new(&[("lib.ts", "export function get() { return { then() {} }; }\n")]);
    let ex = repo.extract("a.ts", "import { get } from './lib';\nexport class A extends B {\n  run() { super.run(); get().then(); }\n}\n");
    assert_eq!(calls(&ex), vec![("sym:a.ts::A.run", "sym:lib.ts::get")]);
}

/// The same corpus from a CRLF checkout: tree-sitter counts `\r\n` as one newline, so rows agree,
/// and every body reaches a node through `lines()`. Pinned rather than argued, because one
/// future slice of raw text by byte offset would break it silently on a Windows checkout only.
#[test]
fn a_crlf_source_extracts_the_same_nodes_and_edges_as_its_lf_twin() {
    let lf = "import { get } from './lib';\n\n/** Считает штраф.\n * FR-PAY-1\n */\n@Injectable()\nexport class Penalty {\n  private rate = 1;\n\n  charge(sum: number) {\n    return get(sum) * this.rate;\n  }\n}\n\nexport function total(x: number) {\n  return x;\n}\n";
    let crlf = lf.replace('\n', "\r\n");
    let repo = Repo::new(&[("lib.ts", "export function get(n: number) { return n; }\n")]);
    let (a, b) = (repo.extract("a.ts", lf), repo.extract("a.ts", &crlf));
    assert_eq!(ids(&a), ["file:a.ts", "sym:a.ts::Penalty", "sym:a.ts::Penalty.rate", "sym:a.ts::Penalty.charge", "sym:a.ts::total", "deco:Injectable"], "two extractions of nothing would agree too");
    assert_eq!(a.nodes, b.nodes);
    assert_eq!(a.edges, b.edges);
    assert!(a.nodes.iter().all(|n| !n.body.contains('\r') && !n.label.contains('\r')));
}

// ---- value uses: DI objects, arrays, decorator arguments, getter reads ----

#[test]
/// A class decorator sits outside the class it decorates, so the file owns what it hands over.
fn identifiers_in_di_objects_and_arrays_are_arg_edges_within_one_file() {
    let ex = extract(
        "identity.module.ts",
        "class Kms {}\nclass Cfg {}\nfunction createKms() { return new Kms(); }\nconst imports = [];\n\
         @Module({ providers: [Cfg, { provide: Kms, useFactory: createKms, inject: [Cfg] }], imports })\n\
         export class IdentityModule {}\n",
    );
    let all = edges(&ex, EdgeKind::Calls);
    let got: Vec<(&str, &str)> = all.iter()
        .filter(|e| e.0 == "file:identity.module.ts").map(|e| (e.1, e.2)).collect();
    assert_eq!(got, vec![
        ("sym:identity.module.ts::Cfg", "arg"),
        ("sym:identity.module.ts::Kms", "arg"),
        ("sym:identity.module.ts::createKms", "arg"),
        ("sym:identity.module.ts::imports", "arg"),
    ], "{all:?}");
}

#[test]
fn a_type_named_in_a_decorator_object_is_an_arg_edge_of_the_decorated_member() {
    let repo = Repo::new(&[("dto.ts", "export class CreateDto {}\n")]);
    let ex = repo.extract(
        "c.ts",
        "import { CreateDto } from './dto';\nexport class C {\n  @ApiBody({ type: CreateDto, examples: { a: { value: [CreateDto] } } })\n  create() {}\n}\n",
    );
    let got: Vec<(&str, &str)> = edges(&ex, EdgeKind::Calls).into_iter().filter(|e| e.1 == "sym:dto.ts::CreateDto").map(|e| (e.0, e.2)).collect();
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].1, "arg");
}

#[test]
fn a_function_inside_an_argument_object_is_its_own_call_site() {
    let ex = extract("a.ts", "function mk() {}\nexport function run() { f({ useFactory: () => mk() }); }\n");
    assert_eq!(edges(&ex, EdgeKind::Calls), vec![("sym:a.ts::run", "sym:a.ts::mk", "")]);
}

#[test]
fn reading_a_getter_of_the_own_class_is_an_edge_and_reading_a_field_is_not() {
    let ex = extract(
        "a.ts",
        "export class A {\n  private n = 1;\n  get secret(): string { return ''; }\n  run() { this.n = 2; return this.secret.length + this.n; }\n}\n",
    );
    assert_eq!(edges(&ex, EdgeKind::Calls), vec![("sym:a.ts::A.run", "sym:a.ts::A.secret", "get")]);
}

#[test]
fn assigning_through_a_setter_is_not_a_read() {
    let ex = extract("a.ts", "export class A {\n  set v(x: number) {}\n  get w() { return 1; }\n  run() { this.v = 1; this.w = 2; }\n}\n");
    assert!(edges(&ex, EdgeKind::Calls).is_empty(), "{:?}", edges(&ex, EdgeKind::Calls));
}

#[test]
fn a_method_named_get_is_not_an_accessor() {
    let ex = extract("a.ts", "export class A {\n  get(k: string) { return k; }\n  run() { return this.get; }\n}\n");
    assert!(edges(&ex, EdgeKind::Calls).is_empty(), "{:?}", edges(&ex, EdgeKind::Calls));
}

#[test]
fn a_getter_read_through_an_injected_field_survives_settle_and_a_field_read_does_not() {
    let tokens = "export class StorageTokens {\n  label = 'x';\n  get secret(): string { return ''; }\n}\n";
    let reader = "import { StorageTokens } from './tokens';\nexport class SignedLinks {\n  constructor(private readonly tokens: StorageTokens) {}\n  sign() { return this.tokens.secret + this.tokens.label; }\n}\n";
    let repo = Repo::new(&[("tokens.ts", tokens), ("signed-links.ts", reader)]);
    let mut g = crate::model::Graph::default();
    g.apply(repo.extract("tokens.ts", tokens));
    g.apply(repo.extract("signed-links.ts", reader));
    g.settle();
    let got: Vec<(&str, &str, &str)> = g.edges.iter().filter(|e| e.kind == EdgeKind::Calls)
        .map(|e| (e.source.as_str(), e.target.as_str(), e.context.as_str())).collect();
    assert_eq!(got, vec![("sym:signed-links.ts::SignedLinks.sign", "sym:tokens.ts::StorageTokens.secret", "get")]);
    assert!(g.pending.is_empty(), "a field read is dropped, not held aside");
}

#[test]
fn a_getter_that_becomes_a_field_loses_its_readers_at_the_next_settle() {
    let reader = "import { T } from './t';\nexport function run(t: T) { return T.now; }\n";
    let repo = Repo::new(&[("t.ts", "export class T { static get now() { return 1; } }\n"), ("r.ts", reader)]);
    let mut g = crate::model::Graph::default();
    g.apply(repo.extract("t.ts", "export class T { static get now() { return 1; } }\n"));
    g.apply(repo.extract("r.ts", reader));
    g.settle();
    assert!(g.edges.iter().any(|e| e.reads_member() && e.target == "sym:t.ts::T.now"));
    g.remove_file("t.ts");
    g.apply(repo.extract("t.ts", "export class T { static now = 1; }\n"));
    g.settle();
    assert!(!g.edges.iter().any(|e| e.reads_member()), "{:?}", g.edges);
}

#[test]
fn a_getter_declared_with_a_space_before_its_parentheses_is_still_an_accessor() {
    assert!(crate::model::declares_accessor("T.secret", "get secret () { return 1; }"));
    assert!(crate::model::declares_accessor("T.secret", "static get secret(): string { return ''; }"));
    assert!(!crate::model::declares_accessor("T.get", "get(k: string) { return k; }"));
}

// ---- the staff fixture ----


fn scan(rel: &str, fixture: &str) -> Extraction {
    let d = tempfile::tempdir().unwrap();
    for f in [
        "apps/api/src/shared/audit/index.ts",
        "apps/api/src/shared/rbac/index.ts",
        "apps/api/src/modules/staff/staff.service.ts",
        "apps/api/src/modules/staff/staff.types.ts",
    ] {
        let p = d.path().join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "export {};\n").unwrap();
    }
    let text = std::fs::read_to_string(format!("{}/tests/fixtures/{fixture}", env!("CARGO_MANIFEST_DIR"))).unwrap();
    CodeExtractor::new(Resolver::new(d.path(), &crate::config::Config::default()).unwrap()).extract(rel, &text)
}

fn has(ex: &Extraction, s: &str, t: &str, k: EdgeKind, ctx: &str) -> bool {
    ex.edges.iter().any(|e| e.source == s && e.target == t && e.kind == k && e.context == ctx)
}

const C: &str = "apps/api/src/modules/staff/staff.controller.ts";

#[test]
fn exports_and_class_members() {
    let ex = scan(C, "staff.controller.ts");
    let ids: Vec<&str> = ex.nodes.iter().map(|n| n.id.as_str()).collect();
    for s in ["StaffController", "helper", "LIMIT", "CreateStaffDto", "Role"] {
        assert!(ids.contains(&format!("sym:{C}::{s}").as_str()), "{s}");
    }
    assert!(ids.contains(&format!("sym:{C}::StaffController.create").as_str()));
    let cls = ex.nodes.iter().find(|n| n.id == format!("sym:{C}::StaffController")).unwrap();
    assert_eq!(cls.kind, NodeKind::Symbol);
    assert_eq!(cls.line, 8);
    assert_eq!(cls.label, "StaffController");
    assert!(has(&ex, &format!("file:{C}"), &format!("sym:{C}::StaffController"), EdgeKind::Declares, "export"));
    assert!(has(&ex, &format!("sym:{C}::StaffController"), &format!("sym:{C}::StaffController.create"), EdgeKind::Declares, ""));
}

#[test]
fn decorators_carry_their_first_string_argument() {
    let ex = scan(C, "staff.controller.ts");
    let m = format!("sym:{C}::StaffController.create");
    assert!(has(&ex, &m, "deco:RequireAction", EdgeKind::DecoratedBy, "staff.manage"));
    assert!(has(&ex, &m, "deco:Audited", EdgeKind::DecoratedBy, ""));
    assert!(has(&ex, &m, "deco:Post", EdgeKind::DecoratedBy, ""));
    assert!(has(&ex, &format!("sym:{C}::StaffController"), "deco:Controller", EdgeKind::DecoratedBy, "staff"));
    assert!(ex.nodes.iter().any(|n| n.id == "deco:RequireAction" && n.kind == NodeKind::Symbol));
}

/// A decorator on a method sits beside it in `class_body`, not inside the
/// `method_definition`; a decorator on a property sits inside `public_field_definition`.
#[test]
fn decorator_on_a_class_property_is_recorded() {
    let ex = scan(C, "staff.controller.ts");
    let p = format!("sym:{C}::StaffController.logger");
    assert!(ex.nodes.iter().any(|n| n.id == p), "property symbol missing");
    assert!(has(&ex, &p, "deco:Inject", EdgeKind::DecoratedBy, "LOGGER"));
}

#[test]
fn imports_and_re_exports_resolve_to_files() {
    let ex = scan(C, "staff.controller.ts");
    assert!(has(&ex, &format!("file:{C}"), "file:apps/api/src/shared/rbac/index.ts", EdgeKind::Imports, "RequireAction"));
    assert!(has(&ex, &format!("file:{C}"), "file:apps/api/src/modules/staff/staff.types.ts", EdgeKind::ReExports, "*"));
    assert!(has(&ex, &format!("file:{C}"), "file:apps/api/src/modules/staff/staff.service.ts", EdgeKind::ReExports, "StaffService"));
    assert!(!ex.edges.iter().any(|e| e.target.contains("nestjs")));
}

#[test]
fn extends_targets_a_symbol_in_the_same_file_when_unresolved() {
    let ex = scan(C, "staff.controller.ts");
    assert!(has(&ex, &format!("sym:{C}::StaffController"), &format!("sym:{C}::BaseController"), EdgeKind::Extends, ""));
}

#[test]
fn non_exported_functions_are_declared_without_export_context() {
    let ex = scan("packages/contracts/src/money.ts", "money.ts");
    assert!(has(&ex, "file:packages/contracts/src/money.ts", "sym:packages/contracts/src/money.ts::asGrosze", EdgeKind::Declares, "export"));
    assert!(has(&ex, "file:packages/contracts/src/money.ts", "sym:packages/contracts/src/money.ts::internal", EdgeKind::Declares, ""));
    let n = ex.nodes.iter().find(|n| n.id.ends_with("::asGrosze")).unwrap();
    assert_eq!(n.body, "Money in grosze; see FR-PAY-03 and INV-11.\nexport function asGrosze(v: number): number { return Math.round(v * 100); }");
}

fn inline(rel: &str, src: &str) -> Extraction {
    let d = tempfile::tempdir().unwrap();
    CodeExtractor::new(Resolver::new(d.path(), &crate::config::Config::default()).unwrap()).extract(rel, src)
}

#[test]
fn a_destructured_object_pattern_declares_each_bound_name_not_the_source_property() {
    let ex = inline("m.ts", "const { x, y: why, ...rest } = obj;\n");
    let ids: Vec<&str> = ex.nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(ids.contains(&"sym:m.ts::x"));
    assert!(ids.contains(&"sym:m.ts::why"));
    assert!(ids.contains(&"sym:m.ts::rest"));
    assert!(!ids.iter().any(|i| i.ends_with("::y")));
}

#[test]
fn an_array_pattern_with_elision_and_a_default_declares_only_the_bound_names() {
    let ex = inline("m.ts", "const [a, , b = 1] = arr;\n");
    let ids: Vec<&str> = ex.nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(ids.contains(&"sym:m.ts::a"));
    assert!(ids.contains(&"sym:m.ts::b"));
    assert_eq!(ids.iter().filter(|i| i.starts_with("sym:")).count(), 2);
}

#[test]
fn a_decorator_group_is_fully_drained_and_never_leaks_to_the_next_method() {
    let ex = inline("m.ts", "class A {\n  @Foo\n  one() {}\n  two() {}\n}\n");
    assert!(has(&ex, "sym:m.ts::A.one", "deco:Foo", EdgeKind::DecoratedBy, ""));
    assert!(!ex.edges.iter().any(|e| e.source == "sym:m.ts::A.two" && e.kind == EdgeKind::DecoratedBy));
}

#[test]
fn a_computed_member_name_that_is_not_a_string_literal_is_skipped() {
    let ex = inline("m.ts", "class A {\n  [Symbol.iterator]() {}\n  bar() {}\n}\n");
    assert!(!ex.nodes.iter().any(|n| n.id.contains("Symbol")));
    assert!(ex.nodes.iter().any(|n| n.id == "sym:m.ts::A.bar"));
}

#[test]
fn a_quoted_member_name_is_unquoted() {
    let ex = inline("m.ts", "class A {\n  'weird-name'() {}\n}\n");
    assert!(ex.nodes.iter().any(|n| n.id == "sym:m.ts::A.weird-name"));
}

#[test]
fn a_non_string_decorator_argument_yields_an_empty_arg() {
    let ex = inline("m.ts", "const SOME_CONST = 1;\nclass A {\n  @Foo(SOME_CONST)\n  bar() {}\n}\n");
    assert!(has(&ex, "sym:m.ts::A.bar", "deco:Foo", EdgeKind::DecoratedBy, ""));
}

#[test]
fn import_equals_require_edge_carries_the_bound_name() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("thing.ts"), "export {};\n").unwrap();
    let ex = CodeExtractor::new(Resolver::new(d.path(), &crate::config::Config::default()).unwrap()).extract("m.ts", "import thing = require('./thing');\n");
    assert!(has(&ex, "file:m.ts", "file:thing.ts", EdgeKind::Imports, "thing"));
}

// ---- id references, one owner each ----


fn refs(rel: &str, src: &str) -> Vec<(String, String, String)> {
    let ex = extract(rel, src);
    ex.edges.iter().filter(|e| e.kind == EdgeKind::References)
        .map(|e| (e.source.clone(), e.target.clone(), e.context.clone())).collect()
}

#[test]
fn comment_ids_attach_to_the_enclosing_symbol() {
    let r = refs("m.ts", "/** see FR-PAY-03 */\nexport function asGrosze() {}\nclass A {\n  // INV-11 here\n  run() { return 'FR-SEC-21'; }\n}\n");
    assert!(r.contains(&("file:m.ts".into(), "FR-PAY-03".into(), "comment".into())));
    assert!(r.contains(&("sym:m.ts::A".into(), "INV-11".into(), "comment".into())));
    assert!(r.contains(&("sym:m.ts::A.run".into(), "FR-SEC-21".into(), "string".into())));
}

#[test]
fn test_titles_count() {
    let r = refs("t.spec.ts", "it('FR-VIS-35 owner sees templates', () => {});\n");
    assert_eq!(r, vec![("file:t.spec.ts".into(), "FR-VIS-35".into(), "string".into())]);
}

#[test]
fn identifiers_are_not_scanned() {
    assert!(refs("x.ts", "const FR_PAY_22 = 1; const a = FRPAY22;\n").is_empty());
}

#[test]
fn template_literal_ids_use_string_context_and_attach_to_method() {
    let r = refs("m.ts", "class A {\n  run() { return `see FR-SEC-21`; }\n}\n");
    assert!(r.contains(&("sym:m.ts::A.run".into(), "FR-SEC-21".into(), "string".into())));
}

#[test]
fn staff_controller_fixture_yields_fr_vis_35() {
    let text = std::fs::read_to_string(format!("{}/tests/fixtures/staff.controller.ts", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let r = refs("c.ts", &text);
    assert!(r.contains(&("file:c.ts".into(), "FR-VIS-35".into(), "comment".into())));
    assert!(r.contains(&("sym:c.ts::helper".into(), "FR-SEC-21".into(), "string".into())));
}

#[test]
fn a_top_level_const_initializer_id_attaches_to_the_variable_symbol() {
    let r = refs("m.ts", "export const LIMIT = 'FR-PAY-01';\n");
    assert_eq!(r, vec![("sym:m.ts::LIMIT".into(), "FR-PAY-01".into(), "string".into())]);
}

#[test]
fn a_nested_function_is_climbed_through_to_its_top_level_owner() {
    let r = refs("m.ts", "function outer() {\n  function inner() {\n    return 'FR-PAY-01';\n  }\n}\n");
    assert_eq!(r, vec![("sym:m.ts::outer".into(), "FR-PAY-01".into(), "string".into())]);
}

#[test]
fn an_id_inside_a_decorator_argument_attaches_to_the_decorated_method() {
    let r = refs("m.ts", "class A {\n  @RequireAction('FR-SEC-21')\n  run() {}\n}\n");
    assert!(r.contains(&("sym:m.ts::A.run".into(), "FR-SEC-21".into(), "string".into())));
}

#[test]
fn interface_body_ids_attach_to_the_interface_symbol() {
    let r = refs("m.ts", "interface X {\n  // FR-PAY-01\n  prop: string;\n}\n");
    assert_eq!(r, vec![("sym:m.ts::X".into(), "FR-PAY-01".into(), "comment".into())]);
}

#[test]
fn two_ids_in_one_string_produce_two_edges_with_the_same_owner() {
    let r = refs("m.ts", "const x = 'see FR-PAY-01 and N-1';\n");
    assert_eq!(r.len(), 2);
    assert!(r.iter().all(|(src, _, ctx)| src == "sym:m.ts::x" && ctx == "string"));
    let targets: Vec<&str> = r.iter().map(|(_, t, _)| t.as_str()).collect();
    assert!(targets.contains(&"FR-PAY-01") && targets.contains(&"N-1"));
}

#[test]
fn an_id_in_a_decorator_above_export_is_the_exported_class_s() {
    let r = refs("m.ts", "@Module({\n  // FR-SEC-30\n  imports: [],\n})\nexport class AppModule {}\n");
    assert_eq!(r, vec![("sym:m.ts::AppModule".into(), "FR-SEC-30".into(), "comment".into())]);
}

#[test]
fn a_call_that_computes_a_destructured_value_is_the_file_s() {
    let repo = Repo::new(&[("lib.ts", "export function load() { return { a: 1, b: 2 }; }\n")]);
    let ex = repo.extract("a.ts", "import { load } from './lib';\nconst { a, b } = load();\nconst c = load();\n");
    let got = edges(&ex, EdgeKind::Calls);
    assert!(got.contains(&("file:a.ts", "sym:lib.ts::load", "")), "{got:?}");
    assert!(got.contains(&("sym:a.ts::c", "sym:lib.ts::load", "")), "{got:?}");
    assert!(!got.iter().any(|e| e.0 == "sym:a.ts::a"), "{got:?}");
}

#[test]
fn a_decorated_field_reads_its_own_line_whatever_the_decorator_spans() {
    let ex = extract("a.ts", "export class A {\n  @Input() name: string;\n  @Column({\n    type: 'int',\n  })\n  age: number;\n}\n");
    let body = |id: &str| ex.nodes.iter().find(|n| n.id == id).map(|n| n.body.clone());
    assert_eq!(body("sym:a.ts::A.name").as_deref(), Some("name: string"));
    assert_eq!(body("sym:a.ts::A.age").as_deref(), Some("age: number"));
}
