;; A file declares what its top-level statements declare, `export`ed, `declare`d or plain; a class
;; and an interface their members, and a binding to an object literal the literal's methods and
;; function-valued properties, which are called through it as a class's are. Nothing in a block or a
;; function body is declared, so a pattern is rooted at the file or starts no deeper than a
;; declarator at the file: depth 2, the one depth every unrooted pattern here shares. A non-exported
;; `namespace` sits in an expression statement and declares nothing, as a non-exported generator
;; does not; a quoted `declare module` is another module's. A member is never exported; a top-level
;; declaration is with its `export` word, or with a later `export { a }`, `export default a` or
;; `export = a`. A quoted member name is the property it spells, which the spec's fold unquotes.
;;
;; Each wrapper a pattern looks through multiplies its compile: a literal reached through `export`,
;; an `as`, a `satisfies` or parentheses cost 60 ms when every wrapper nested in every other, and no
;; input binds a literal through two of them.

;; A binding to a literal holds its members, so it is declared ahead of the plain binding below.

((variable_declarator name: (identifier) @name value: [(object) (as_expression (object)) (satisfies_expression (object)) (parenthesized_expression (object))]) @definition.object (#set! depth "2"))
((export_statement "export" @modifier declaration: (lexical_declaration (variable_declarator name: (identifier) @name value: [(object) (as_expression (object)) (satisfies_expression (object)) (parenthesized_expression (object))]) @definition.object)) (#set! depth "2"))

(program [
  (function_declaration name: (identifier) @name) @definition.function
  (function_signature name: (identifier) @name) @definition.function
  (class_declaration name: (type_identifier) @name) @definition.class
  (abstract_class_declaration name: (type_identifier) @name) @definition.class
  (interface_declaration name: (type_identifier) @name) @definition.interface
  (type_alias_declaration name: (type_identifier) @name) @definition.type
  (enum_declaration name: (identifier) @name) @definition.enum
  (lexical_declaration (variable_declarator name: (identifier) @name) @definition.variable)
  (lexical_declaration (variable_declarator name: (object_pattern [(shorthand_property_identifier_pattern) @name (pair_pattern value: (identifier) @name) (rest_pattern (identifier) @name) (object_assignment_pattern left: (shorthand_property_identifier_pattern) @name) (pair_pattern value: (assignment_pattern left: (identifier) @name))])) @definition.variable)
  (lexical_declaration (variable_declarator name: (array_pattern [(identifier) @name (assignment_pattern left: (identifier) @name) (rest_pattern (identifier) @name)])) @definition.variable)
  (variable_declaration (variable_declarator name: (identifier) @name) @definition.variable)
  (variable_declaration (variable_declarator name: (object_pattern [(shorthand_property_identifier_pattern) @name (pair_pattern value: (identifier) @name) (rest_pattern (identifier) @name) (object_assignment_pattern left: (shorthand_property_identifier_pattern) @name) (pair_pattern value: (assignment_pattern left: (identifier) @name))])) @definition.variable)
  (variable_declaration (variable_declarator name: (array_pattern [(identifier) @name (assignment_pattern left: (identifier) @name) (rest_pattern (identifier) @name)])) @definition.variable)
])

(program (export_statement "export" @modifier declaration: [
    (function_declaration name: (identifier) @name) @definition.function
    (function_signature name: (identifier) @name) @definition.function
    (class_declaration name: (type_identifier) @name) @definition.class
    (abstract_class_declaration name: (type_identifier) @name) @definition.class
    (interface_declaration name: (type_identifier) @name) @definition.interface
    (type_alias_declaration name: (type_identifier) @name) @definition.type
    (enum_declaration name: (identifier) @name) @definition.enum
    (lexical_declaration (variable_declarator name: (identifier) @name) @definition.variable)
    (lexical_declaration (variable_declarator name: (object_pattern [(shorthand_property_identifier_pattern) @name (pair_pattern value: (identifier) @name) (rest_pattern (identifier) @name) (object_assignment_pattern left: (shorthand_property_identifier_pattern) @name) (pair_pattern value: (assignment_pattern left: (identifier) @name))])) @definition.variable)
    (lexical_declaration (variable_declarator name: (array_pattern [(identifier) @name (assignment_pattern left: (identifier) @name) (rest_pattern (identifier) @name)])) @definition.variable)
    (variable_declaration (variable_declarator name: (identifier) @name) @definition.variable)
    (variable_declaration (variable_declarator name: (object_pattern [(shorthand_property_identifier_pattern) @name (pair_pattern value: (identifier) @name) (rest_pattern (identifier) @name) (object_assignment_pattern left: (shorthand_property_identifier_pattern) @name) (pair_pattern value: (assignment_pattern left: (identifier) @name))])) @definition.variable)
    (variable_declaration (variable_declarator name: (array_pattern [(identifier) @name (assignment_pattern left: (identifier) @name) (rest_pattern (identifier) @name)])) @definition.variable)
    (internal_module name: [(identifier) (nested_identifier)] @name) @definition.module
    (generator_function_declaration name: (identifier) @name) @definition.function
  ]))

(program (ambient_declaration [
    (function_declaration name: (identifier) @name) @definition.function
    (function_signature name: (identifier) @name) @definition.function
    (class_declaration name: (type_identifier) @name) @definition.class
    (abstract_class_declaration name: (type_identifier) @name) @definition.class
    (interface_declaration name: (type_identifier) @name) @definition.interface
    (type_alias_declaration name: (type_identifier) @name) @definition.type
    (enum_declaration name: (identifier) @name) @definition.enum
    (lexical_declaration (variable_declarator name: (identifier) @name) @definition.variable)
    (lexical_declaration (variable_declarator name: (object_pattern [(shorthand_property_identifier_pattern) @name (pair_pattern value: (identifier) @name) (rest_pattern (identifier) @name) (object_assignment_pattern left: (shorthand_property_identifier_pattern) @name) (pair_pattern value: (assignment_pattern left: (identifier) @name))])) @definition.variable)
    (lexical_declaration (variable_declarator name: (array_pattern [(identifier) @name (assignment_pattern left: (identifier) @name) (rest_pattern (identifier) @name)])) @definition.variable)
    (variable_declaration (variable_declarator name: (identifier) @name) @definition.variable)
    (variable_declaration (variable_declarator name: (object_pattern [(shorthand_property_identifier_pattern) @name (pair_pattern value: (identifier) @name) (rest_pattern (identifier) @name) (object_assignment_pattern left: (shorthand_property_identifier_pattern) @name) (pair_pattern value: (assignment_pattern left: (identifier) @name))])) @definition.variable)
    (variable_declaration (variable_declarator name: (array_pattern [(identifier) @name (assignment_pattern left: (identifier) @name) (rest_pattern (identifier) @name)])) @definition.variable)
    (internal_module name: [(identifier) (nested_identifier)] @name) @definition.module
    (module name: (identifier) @name) @definition.module
  ]))

(program (export_statement "export" @modifier declaration: (ambient_declaration [
    (function_declaration name: (identifier) @name) @definition.function
    (function_signature name: (identifier) @name) @definition.function
    (class_declaration name: (type_identifier) @name) @definition.class
    (abstract_class_declaration name: (type_identifier) @name) @definition.class
    (interface_declaration name: (type_identifier) @name) @definition.interface
    (type_alias_declaration name: (type_identifier) @name) @definition.type
    (enum_declaration name: (identifier) @name) @definition.enum
    (lexical_declaration (variable_declarator name: (identifier) @name) @definition.variable)
    (lexical_declaration (variable_declarator name: (object_pattern [(shorthand_property_identifier_pattern) @name (pair_pattern value: (identifier) @name) (rest_pattern (identifier) @name) (object_assignment_pattern left: (shorthand_property_identifier_pattern) @name) (pair_pattern value: (assignment_pattern left: (identifier) @name))])) @definition.variable)
    (lexical_declaration (variable_declarator name: (array_pattern [(identifier) @name (assignment_pattern left: (identifier) @name) (rest_pattern (identifier) @name)])) @definition.variable)
    (variable_declaration (variable_declarator name: (identifier) @name) @definition.variable)
    (variable_declaration (variable_declarator name: (object_pattern [(shorthand_property_identifier_pattern) @name (pair_pattern value: (identifier) @name) (rest_pattern (identifier) @name) (object_assignment_pattern left: (shorthand_property_identifier_pattern) @name) (pair_pattern value: (assignment_pattern left: (identifier) @name))])) @definition.variable)
    (variable_declaration (variable_declarator name: (array_pattern [(identifier) @name (assignment_pattern left: (identifier) @name) (rest_pattern (identifier) @name)])) @definition.variable)
    (internal_module name: [(identifier) (nested_identifier)] @name) @definition.module
    (module name: (identifier) @name) @definition.module
  ])))

;; Members, of a statement at the file, behind `export` or behind `declare`, and behind both, where
;; `declare` keeps the class at depth 3. A class in a body starts deeper, and an anonymous default class is a `class`, which declares nothing.
;; An interface's data property is declared as a class's field is, though nothing calls it: a props
;; or a record interface is mostly data, and a hunk that edits one property must name that property.

((class_declaration body: (class_body [
    (method_definition name: [(property_identifier) (private_property_identifier) (number) (string) (computed_property_name (string))] @name) @definition.method
    (abstract_method_signature name: [(property_identifier) (private_property_identifier) (number) (string) (computed_property_name (string))] @name) @definition.method
    (public_field_definition name: [(property_identifier) (private_property_identifier) (number) (string) (computed_property_name (string))] @name) @definition.field
  ])) (#set! depth "2"))

((abstract_class_declaration body: (class_body [
    (method_definition name: [(property_identifier) (private_property_identifier) (number) (string) (computed_property_name (string))] @name) @definition.method
    (abstract_method_signature name: [(property_identifier) (private_property_identifier) (number) (string) (computed_property_name (string))] @name) @definition.method
    (public_field_definition name: [(property_identifier) (private_property_identifier) (number) (string) (computed_property_name (string))] @name) @definition.field
  ])) (#set! depth "2"))

((interface_declaration body: (interface_body [
    (method_signature name: [(property_identifier) (private_property_identifier) (number) (string) (computed_property_name (string))] @name) @definition.method
    (property_signature name: [(property_identifier) (private_property_identifier) (number) (string) (computed_property_name (string))] @name) @definition.property
  ])) (#set! depth "2"))

((ambient_declaration [
  (class_declaration body: (class_body [
    (method_definition name: [(property_identifier) (private_property_identifier) (number) (string) (computed_property_name (string))] @name) @definition.method
    (abstract_method_signature name: [(property_identifier) (private_property_identifier) (number) (string) (computed_property_name (string))] @name) @definition.method
    (public_field_definition name: [(property_identifier) (private_property_identifier) (number) (string) (computed_property_name (string))] @name) @definition.field
  ]))
  (abstract_class_declaration body: (class_body [
    (method_definition name: [(property_identifier) (private_property_identifier) (number) (string) (computed_property_name (string))] @name) @definition.method
    (abstract_method_signature name: [(property_identifier) (private_property_identifier) (number) (string) (computed_property_name (string))] @name) @definition.method
    (public_field_definition name: [(property_identifier) (private_property_identifier) (number) (string) (computed_property_name (string))] @name) @definition.field
  ]))
  (interface_declaration body: (interface_body [
    (method_signature name: [(property_identifier) (private_property_identifier) (number) (string) (computed_property_name (string))] @name) @definition.method
    (property_signature name: [(property_identifier) (private_property_identifier) (number) (string) (computed_property_name (string))] @name) @definition.property
  ]))
]) (#set! depth "2"))

;; A literal's members, under a plain or an exported binding.

((variable_declarator name: (identifier) value: [(object [
    (method_definition name: [(property_identifier) (private_property_identifier) (number) (string) (computed_property_name (string))] @name) @definition.method
    (pair key: [(property_identifier) (number) (string) (computed_property_name (string))] @name value: [(arrow_function) (function_expression) (generator_function)]) @definition.method
  ]) (as_expression (object [
    (method_definition name: [(property_identifier) (private_property_identifier) (number) (string) (computed_property_name (string))] @name) @definition.method
    (pair key: [(property_identifier) (number) (string) (computed_property_name (string))] @name value: [(arrow_function) (function_expression) (generator_function)]) @definition.method
  ])) (satisfies_expression (object [
    (method_definition name: [(property_identifier) (private_property_identifier) (number) (string) (computed_property_name (string))] @name) @definition.method
    (pair key: [(property_identifier) (number) (string) (computed_property_name (string))] @name value: [(arrow_function) (function_expression) (generator_function)]) @definition.method
  ])) (parenthesized_expression (object [
    (method_definition name: [(property_identifier) (private_property_identifier) (number) (string) (computed_property_name (string))] @name) @definition.method
    (pair key: [(property_identifier) (number) (string) (computed_property_name (string))] @name value: [(arrow_function) (function_expression) (generator_function)]) @definition.method
  ]))]) (#set! depth "2"))

((export_statement declaration: (lexical_declaration (variable_declarator name: (identifier) value: [(object [
    (method_definition name: [(property_identifier) (private_property_identifier) (number) (string) (computed_property_name (string))] @name) @definition.method
    (pair key: [(property_identifier) (number) (string) (computed_property_name (string))] @name value: [(arrow_function) (function_expression) (generator_function)]) @definition.method
  ]) (as_expression (object [
    (method_definition name: [(property_identifier) (private_property_identifier) (number) (string) (computed_property_name (string))] @name) @definition.method
    (pair key: [(property_identifier) (number) (string) (computed_property_name (string))] @name value: [(arrow_function) (function_expression) (generator_function)]) @definition.method
  ])) (satisfies_expression (object [
    (method_definition name: [(property_identifier) (private_property_identifier) (number) (string) (computed_property_name (string))] @name) @definition.method
    (pair key: [(property_identifier) (number) (string) (computed_property_name (string))] @name value: [(arrow_function) (function_expression) (generator_function)]) @definition.method
  ])) (parenthesized_expression (object [
    (method_definition name: [(property_identifier) (private_property_identifier) (number) (string) (computed_property_name (string))] @name) @definition.method
    (pair key: [(property_identifier) (number) (string) (computed_property_name (string))] @name value: [(arrow_function) (function_expression) (generator_function)]) @definition.method
  ]))]))) (#set! depth "2"))

;; Names exported apart from their declaration.
(program (export_statement !source (export_clause (export_specifier name: (identifier) @export.name))))
(program (export_statement (identifier) @export.name))
