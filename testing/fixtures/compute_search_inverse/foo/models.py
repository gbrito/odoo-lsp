# ============================================================
# Test Fixture for compute=, search=, inverse= Field Attributes
# Comprehensive tests for: completions, diagnostics, go-to-definition
# ============================================================


# ============================================================
# SECTION 1: Basic Model with Methods
# Tests diagnostics, go-to-definition, and valid method recognition
# ============================================================


class TestModel(Model):
    _name = "test.model"

    name = fields.Char()
    value = fields.Integer()

    # --------------------------------------------------------
    # DIAGNOSTIC TESTS - Invalid method names produce errors
    # --------------------------------------------------------

    invalid_compute = fields.Char(compute="_nonexistent")
    #                                      ^diag Model `test.model` has no method `_nonexistent`

    invalid_search = fields.Char(search="_bad_search")
    #                                    ^diag Model `test.model` has no method `_bad_search`

    invalid_inverse = fields.Char(inverse="_wrong_inverse")
    #                                      ^diag Model `test.model` has no method `_wrong_inverse`

    # --------------------------------------------------------
    # GO-TO-DEFINITION TESTS - Valid method names navigate to definition
    # --------------------------------------------------------

    gotodef_compute = fields.Char(compute="_compute_name")
    #                                      ^def

    gotodef_search = fields.Char(search="_search_name")
    #                                    ^def

    gotodef_inverse = fields.Char(inverse="_inverse_name")
    #                                      ^def

    # --------------------------------------------------------
    # VALID METHODS - No diagnostics should appear (implicit test)
    # These use valid method names that exist in the model
    # --------------------------------------------------------

    valid_compute = fields.Char(compute="_compute_name")
    valid_search = fields.Char(search="_search_name")
    valid_inverse = fields.Char(inverse="_inverse_name")

    # Also test compute="_compute_value" - another valid method
    valid_compute2 = fields.Char(compute="_compute_value")

    # --------------------------------------------------------
    # METHOD DEFINITIONS
    # --------------------------------------------------------

    def _compute_name(self):
        for record in self:
            record.name = "computed"

    def _compute_value(self):
        for record in self:
            record.value = 42

    def _search_name(self, operator, value):
        return [("name", operator, value)]

    def _inverse_name(self):
        pass

    def action_do(self):
        pass


# ============================================================
# SECTION 2: Inheritance Tests
# Methods from parent models should be recognized as valid
# ============================================================


class BaseModel(Model):
    _name = "base.model"

    name = fields.Char()

    def _compute_base_name(self):
        pass

    def base_action(self):
        pass


class DerivedModel(Model):
    _name = "derived.model"
    _inherit = "base.model"

    # Using base method - should NOT produce diagnostic (method exists in parent)
    uses_base = fields.Char(compute="_compute_base_name")

    # Using non-existent method - SHOULD produce diagnostic
    bad_derived = fields.Char(compute="_missing_method")
    #                                  ^diag Model `derived.model` has no method `_missing_method`

    def _compute_derived(self):
        pass


# ============================================================
# SECTION 3: Multiple Attributes on Same Field
# Common Odoo pattern: compute + inverse + search
# ============================================================


class CombinedField(Model):
    _name = "combined.field"

    name = fields.Char()

    # --------------------------------------------------------
    # GO-TO-DEFINITION TESTS - For non-first class in file
    # This tests that go-to-definition works in subsequent model classes
    # --------------------------------------------------------

    gotodef_compute_multi = fields.Char(compute="_compute_full")
    #                                            ^def

    gotodef_inverse_multi = fields.Char(inverse="_inverse_full")
    #                                            ^def

    gotodef_search_multi = fields.Char(search="_search_full")
    #                                          ^def

    # Field with all three attributes - all valid (no diagnostics)
    full_field = fields.Char(
        compute="_compute_full",
        inverse="_inverse_full",
        search="_search_full",
    )

    # Invalid methods in each position - produce diagnostics
    bad_compute = fields.Char(compute="_bad_compute")
    #                                  ^diag Model `combined.field` has no method `_bad_compute`

    bad_inverse = fields.Char(inverse="_bad_inverse")
    #                                  ^diag Model `combined.field` has no method `_bad_inverse`

    bad_search = fields.Char(search="_bad_search")
    #                                ^diag Model `combined.field` has no method `_bad_search`

    def _compute_full(self):
        pass

    def _inverse_full(self):
        pass

    def _search_full(self, operator, value):
        return []


# ============================================================
# SECTION 4: Related Fields
# Tests for related= field path validation
# ============================================================


class Partner(Model):
    _name = "res.partner"

    name = fields.Char()
    country_id = fields.Many2one("res.country")
    company_id = fields.Many2one("res.company")


class Country(Model):
    _name = "res.country"

    name = fields.Char()
    code = fields.Char()


class Company(Model):
    _name = "res.company"

    name = fields.Char()
    partner_id = fields.Many2one("res.partner")
    country_id = fields.Many2one("res.country")


class RelatedFieldTest(Model):
    _name = "related.field.test"

    partner_id = fields.Many2one("res.partner")

    # --------------------------------------------------------
    # VALID RELATED PATHS - No diagnostics
    # --------------------------------------------------------

    # Simple related field (single hop)
    partner_name = fields.Char(related="partner_id.name")

    # Two-hop related field
    partner_country = fields.Many2one(related="partner_id.country_id")

    # Three-hop related field
    partner_country_name = fields.Char(related="partner_id.country_id.name")
    partner_country_code = fields.Char(related="partner_id.country_id.code")

    # --------------------------------------------------------
    # INVALID RELATED PATHS - Should produce diagnostics
    # --------------------------------------------------------

    # Non-existent field on partner
    bad_related = fields.Char(related="partner_id.nonexistent")
    #                                             ^diag Model `res.partner` has no field `nonexistent`

    # Non-existent intermediate field - reports field.remaining_path as not relational
    bad_path = fields.Char(related="partner_id.bad_field.name")
    #                                          ^diag `bad_field.name` is not a relational field

    # Non-relational field used in path (country_id.name is valid, name.something is not)
    bad_non_rel = fields.Char(related="partner_id.name.something")
    #                                             ^diag `name.something` is not a relational field

    # --------------------------------------------------------
    # GO-TO-DEFINITION for related fields
    # --------------------------------------------------------

    gotodef_related = fields.Char(related="partner_id.name")
    #                                      ^def


# ============================================================
# SECTION 5: @api.depends_context Decorator
# Tests that @api.depends_context is recognized without errors
# Note: The string arguments are context keys, NOT fields - they are not validated
# ============================================================


class ContextDependentModel(Model):
    _name = "context.dependent"

    company_id = fields.Many2one("res.company")
    name = fields.Char()

    # --------------------------------------------------------
    # Basic @api.depends_context - no validation of context keys
    # --------------------------------------------------------

    @api.depends_context("company")
    def _compute_company_dependent(self):
        pass

    # --------------------------------------------------------
    # Multiple context dependencies
    # --------------------------------------------------------

    @api.depends_context("uid", "company", "lang")
    def _compute_multi_context(self):
        pass

    # --------------------------------------------------------
    # Combined with @api.depends - both should work
    # --------------------------------------------------------

    @api.depends_context("company")
    @api.depends("company_id", "name")
    def _compute_combined(self):
        pass

    # --------------------------------------------------------
    # Using the computed methods in field definitions
    # Go-to-definition should work for these
    # --------------------------------------------------------

    computed_value = fields.Char(compute="_compute_company_dependent")
    #                                     ^def

    combined_value = fields.Char(compute="_compute_combined")
    #                                     ^def

    # Invalid method - should show diagnostic
    invalid_context = fields.Char(compute="_missing_context_method")
    #                                      ^diag Model `context.dependent` has no method `_missing_context_method`


# ============================================================
# SECTION 6: Deep Dotted Paths in @api.depends
# Tests for 4+ level field paths and stacked decorators
# ============================================================


class Order(Model):
    _name = "test.order"

    name = fields.Char()
    company_id = fields.Many2one("res.company")
    partner_id = fields.Many2one("res.partner")

    # --------------------------------------------------------
    # Four-level dotted path (valid)
    # order -> company -> partner -> country -> name
    # --------------------------------------------------------

    @api.depends("company_id.partner_id.country_id.name")
    def _compute_four_level_valid(self):
        pass

    four_level_field = fields.Char(compute="_compute_four_level_valid")

    # --------------------------------------------------------
    # Four-level dotted path (invalid at 4th level)
    # --------------------------------------------------------

    @api.depends("company_id.partner_id.country_id.nonexistent")
    #                                              ^diag Model `res.country` has no field `nonexistent`
    def _compute_four_level_invalid(self):
        pass

    # --------------------------------------------------------
    # Five-level dotted path (valid through self-reference)
    # order -> partner -> company -> partner -> country -> code
    # --------------------------------------------------------

    @api.depends("partner_id.company_id.partner_id.country_id.code")
    def _compute_five_level_valid(self):
        pass

    five_level_field = fields.Char(compute="_compute_five_level_valid")

    # --------------------------------------------------------
    # Stacked @api.depends decorators
    # --------------------------------------------------------

    @api.depends("company_id.name")
    @api.depends("partner_id.name")
    def _compute_stacked_depends(self):
        pass

    stacked_field = fields.Char(compute="_compute_stacked_depends")
    #                                    ^def

    # --------------------------------------------------------
    # Multiple fields in single @api.depends, mixed valid/invalid
    # --------------------------------------------------------

    @api.depends("name", "partner_id.name", "company_id.bad_field")
    #                                                   ^diag Model `res.company` has no field `bad_field`
    def _compute_mixed_depends(self):
        pass

    # --------------------------------------------------------
    # Path through non-relational field (should error early)
    # --------------------------------------------------------

    @api.depends("name.something")
    #             ^diag `name.something` is not a relational field
    def _compute_non_relational_path(self):
        pass

    # --------------------------------------------------------
    # Empty string in @api.depends (edge case)
    # --------------------------------------------------------

    @api.depends("")
    def _compute_empty_depends(self):
        # Empty depends is unusual but technically valid
        pass

    empty_depends_field = fields.Char(compute="_compute_empty_depends")
