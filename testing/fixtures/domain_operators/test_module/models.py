# Test cases for domain operator validation and subdomain support


class TestDomains(Model):
    _name = "test.domains"

    # ===========================================
    # Test: Valid operators - no diagnostics expected
    # ===========================================

    # Basic comparison operators
    valid_eq = fields.Many2one("res.partner", domain=[("name", "=", "test")])
    #                                                    ^complete name
    valid_neq = fields.Many2one("res.partner", domain=[("name", "!=", "test")])
    valid_lt = fields.Many2one("res.partner", domain=[("age", "<", 18)])
    valid_lte = fields.Many2one("res.partner", domain=[("age", "<=", 18)])
    valid_gt = fields.Many2one("res.partner", domain=[("age", ">", 18)])
    valid_gte = fields.Many2one("res.partner", domain=[("age", ">=", 18)])

    # Pattern matching operators
    valid_like = fields.Many2one("res.partner", domain=[("name", "like", "test")])
    valid_ilike = fields.Many2one("res.partner", domain=[("name", "ilike", "test")])
    valid_not_like = fields.Many2one(
        "res.partner", domain=[("name", "not like", "test")]
    )
    valid_not_ilike = fields.Many2one(
        "res.partner", domain=[("name", "not ilike", "test")]
    )
    valid_eqlike = fields.Many2one("res.partner", domain=[("name", "=like", "test%")])
    valid_eqilike = fields.Many2one("res.partner", domain=[("name", "=ilike", "test%")])

    # Set operators
    valid_in = fields.Many2one("res.partner", domain=[("name", "in", ["a", "b"])])
    valid_not_in = fields.Many2one(
        "res.partner", domain=[("name", "not in", ["a", "b"])]
    )

    # Hierarchical operators
    valid_child_of = fields.Many2one(
        "res.partner", domain=[("parent_id", "child_of", 1)]
    )
    valid_parent_of = fields.Many2one(
        "res.partner", domain=[("parent_id", "parent_of", 1)]
    )

    # Optional equals operator
    valid_eq_optional = fields.Many2one("res.partner", domain=[("name", "=?", False)])

    # ===========================================
    # Test: Invalid operator - should show diagnostic
    # ===========================================

    invalid_op = fields.Many2one("res.partner", domain=[("name", "foo", "test")])
    #                                                             ^diag Invalid domain operator `foo`. Valid operators: !=, <, <=, =, =?, =ilike, =like, >, >=, any, child_of, ilike, in, like, not any, not ilike, not in, not like, parent_of

    invalid_op2 = fields.Many2one("res.partner", domain=[("name", "equals", "test")])
    #                                                              ^diag Invalid domain operator `equals`. Valid operators: !=, <, <=, =, =?, =ilike, =like, >, >=, any, child_of, ilike, in, like, not any, not ilike, not in, not like, parent_of

    invalid_op3 = fields.Many2one("res.partner", domain=[("name", "contains", "test")])
    #                                                              ^diag Invalid domain operator `contains`. Valid operators: !=, <, <=, =, =?, =ilike, =like, >, >=, any, child_of, ilike, in, like, not any, not ilike, not in, not like, parent_of

    # ===========================================
    # Test: any/not any with subdomain - valid cases
    # ===========================================

    # Basic any - completions should show res.partner fields since child_ids relates to res.partner
    any_valid = fields.Many2one(
        "res.partner",
        domain=[
            ("child_ids", "any", [("name", "=", "John")])
            #                       ^complete active age child_ids country_id name parent_id state tag_ids
        ],
    )

    # not any
    not_any_valid = fields.Many2one(
        "res.partner",
        domain=[
            ("tag_ids", "not any", [("color", ">", 5)])
            #                         ^complete color name
        ],
    )

    # Completions in subdomain with nested field path
    any_nested_path = fields.Many2one(
        "res.partner",
        domain=[
            ("child_ids", "any", [("country_id.code", "=", "US")])
            #                       ^complete active age child_ids country_id name parent_id state tag_ids
        ],
    )

    # ===========================================
    # Test: any with non-relational field - should show diagnostic
    # ===========================================

    any_non_rel = fields.Many2one(
        "res.partner",
        domain=[
            ("name", "any", [("foo", "=", "bar")])
            #        ^diag Operator `any` can only be used with relational fields (Many2one, One2many, Many2many), but `name` is not relational
        ],
    )

    any_non_rel2 = fields.Many2one(
        "res.partner",
        domain=[
            ("age", "not any", [("foo", "=", "bar")])
            #       ^diag Operator `not any` can only be used with relational fields (Many2one, One2many, Many2many), but `age` is not relational
        ],
    )

    # ===========================================
    # Test: any with non-list value - should show diagnostic
    # ===========================================

    any_non_list = fields.Many2one(
        "res.partner",
        domain=[
            ("child_ids", "any", "not_a_list")
            #                    ^diag Operator `any` requires a domain list as value, got string
        ],
    )

    # ===========================================
    # Test: Nested subdomains (multi-level)
    # ===========================================

    # Two-level nesting: partner -> child (partner) -> country
    nested_any = fields.Many2one(
        "res.partner",
        domain=[
            (
                "child_ids",
                "any",
                [
                    ("country_id", "any", [("code", "=", "US")])
                    #                       ^complete code name
                ],
            )
        ],
    )

    # Invalid field in nested subdomain
    nested_invalid_field = fields.Many2one(
        "res.partner",
        domain=[
            ("child_ids", "any", [("nonexistent", "=", "test")])
            #                       ^diag Model `res.partner` has no field `nonexistent`
        ],
    )

    # Invalid operator in nested subdomain
    nested_invalid_op = fields.Many2one(
        "res.partner",
        domain=[
            ("child_ids", "any", [("name", "bad_op", "test")])
            #                               ^diag Invalid domain operator `bad_op`. Valid operators: !=, <, <=, =, =?, =ilike, =like, >, >=, any, child_of, ilike, in, like, not any, not ilike, not in, not like, parent_of
        ],
    )

    # ===========================================
    # Test: Operator-field type warnings
    # ===========================================

    # Using 'like' on an Integer field is unusual (but valid)
    warn_like_integer = fields.Many2one("res.partner", domain=[("age", "like", "18")])
    #                                                                   ^diag Operator `like` is unusual for Integer field `age`

    # Using 'child_of' on a non-relational field is unusual
    warn_hierarchy_char = fields.Many2one(
        "res.partner",
        domain=[("name", "child_of", 1)],
        #                 ^diag Operator `child_of` is unusual for Char field `name`
    )

    # Using '<' on a Selection field is unusual
    warn_lt_selection = fields.Many2one("res.partner", domain=[("state", "<", "draft")])
    #                                                                     ^diag Operator `<` is unusual for Selection field `state`

    # Using 'any' on a non-relational field (already tested above, but validates warning)
    # The existing test already covers 'any' on non-relational triggering error not warning

    # ===========================================
    # Test: Domain structure validation (Polish notation)
    # ===========================================

    # Missing operand for '|' operator - needs 2 terms but only has 1
    struct_missing_operand = fields.Many2one(
        "res.partner",
        domain=["|", ("name", "=", "test")],
        #      ^diag Domain is syntactically incorrect: 1 more term(s) expected
    )

    # Missing operand for '&' operator
    struct_missing_and_operand = fields.Many2one(
        "res.partner",
        domain=["&", ("name", "=", "test")],
        #      ^diag Domain is syntactically incorrect: 1 more term(s) expected
    )

    # Correct structure: '|' with 2 terms
    struct_valid_or = fields.Many2one(
        "res.partner",
        domain=["|", ("name", "=", "John"), ("name", "=", "Jane")],
    )

    # Correct structure: '&' with 2 terms (explicit)
    struct_valid_and = fields.Many2one(
        "res.partner",
        domain=["&", ("name", "=", "test"), ("active", "=", True)],
    )

    # Correct structure: '!' with 1 term
    struct_valid_not = fields.Many2one(
        "res.partner",
        domain=["!", ("active", "=", False)],
    )

    # Complex valid structure: '|' ('&' A B) C
    struct_valid_complex = fields.Many2one(
        "res.partner",
        domain=["|", "&", ("name", "=", "A"), ("age", ">", 18), ("active", "=", True)],
    )

    # ===========================================
    # Test: Value type validation for 'in' and 'not in'
    # ===========================================

    # Valid: 'in' with a list value
    value_in_list_valid = fields.Many2one(
        "res.partner",
        domain=[("name", "in", ["John", "Jane"])],
    )

    # Valid: 'not in' with a list value
    value_not_in_list_valid = fields.Many2one(
        "res.partner",
        domain=[("name", "not in", ["blocked", "banned"])],
    )

    # Invalid: 'in' with a string value
    value_in_string_invalid = fields.Many2one(
        "res.partner",
        domain=[("name", "in", "John")],
        #                      ^diag Operator `in` requires a list or tuple value, got string
    )

    # Invalid: 'not in' with an integer value
    value_not_in_int_invalid = fields.Many2one(
        "res.partner",
        domain=[("age", "not in", 18)],
        #                         ^diag Operator `not in` requires a list or tuple value, got integer
    )

    # ===========================================
    # Test: Three-level nested any (deep nesting)
    # ===========================================

    # Triple nesting: partner -> child (partner) -> child (partner) -> name
    triple_nested_any = fields.Many2one(
        "res.partner",
        domain=[
            (
                "child_ids",
                "any",
                [
                    (
                        "child_ids",
                        "any",
                        [("name", "ilike", "test")]
                        #  ^complete active age child_ids country_id name parent_id state tag_ids
                    )
                ],
            )
        ],
    )

    # ===========================================
    # Test: =? with various value types (common Odoo pattern)
    # ===========================================

    # =? with False - very common pattern
    optional_eq_with_false = fields.Many2one(
        "res.partner",
        domain=[("country_id", "=?", False)]
    )

    # =? with None equivalent
    optional_eq_with_none = fields.Many2one(
        "res.partner",
        domain=[("parent_id", "=?", None)]
    )

    # ===========================================
    # Test: child_of/parent_of on self-referential field
    # ===========================================

    # child_of on self-referential Many2one (common pattern)
    self_ref_child_of = fields.Many2one(
        "res.partner",
        domain=[("id", "child_of", "parent_id")]
    )

    # parent_of on self-referential field
    self_ref_parent_of = fields.Many2one(
        "res.partner",
        domain=[("id", "parent_of", "child_ids")]
    )

    # ===========================================
    # Test: Combined boolean operators with any
    # ===========================================

    # OR with any subdomains
    or_with_any = fields.Many2one(
        "res.partner",
        domain=[
            "|",
            ("child_ids", "any", [("active", "=", True)]),
            ("tag_ids", "any", [("color", ">", 0)])
        ],
    )

    # NOT with any
    not_with_any = fields.Many2one(
        "res.partner",
        domain=[
            "!",
            ("child_ids", "any", [("active", "=", False)])
        ],
    )

    # Complex: (A OR (B AND any_C))
    complex_boolean_any = fields.Many2one(
        "res.partner",
        domain=[
            "|",
            ("active", "=", True),
            "&",
            ("age", ">", 18),
            ("child_ids", "any", [("name", "ilike", "VIP")])
        ],
    )
