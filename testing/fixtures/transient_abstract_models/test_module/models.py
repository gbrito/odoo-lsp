# ============================================================
# Test Fixture for TransientModel and AbstractModel
# Tests base class type recognition and security rules behavior
# ============================================================


# ============================================================
# SECTION 1: TransientModel Tests
# TransientModel is for wizards - still requires access rules
# Note: Only ONE model per file can trigger "no access rules" diagnostic
# so we add access rules for TransientModel to test it separately
# ============================================================


class TestWizard(models.TransientModel):
    _name = "test.wizard"
    # Access rules are defined for test.wizard in security/ir.model.access.csv
    # So NO diagnostic here - this tests that TransientModel IS indexed

    name = fields.Char()
    value = fields.Integer()

    def action_confirm(self):
        # TransientModel should have field completions
        self.mapped("name")
        #            ^complete name value


# ============================================================
# SECTION 2: AbstractModel Tests
# AbstractModel is a mixin - NO access rules required (not persisted)
# ============================================================


class TestMixin(models.AbstractModel):
    _name = "test.mixin"
    # NO diagnostic here - AbstractModel cannot have access rules (skipped by design)

    mixin_field = fields.Char()
    mixin_value = fields.Integer()

    def mixin_method(self):
        # AbstractModel should also have field completions
        self.mapped("mixin_field")
        #            ^complete mixin_field mixin_value


# ============================================================
# SECTION 3: Regular Model Using AbstractModel Mixin
# Model inheriting from AbstractModel should work normally
# ============================================================


class ConcreteModel(models.Model):
    """Model that has access rules defined"""

    _name = "concrete.model"
    _inherit = "test.mixin"

    concrete_field = fields.Char()

    def use_mixin(self):
        # Should have access to both mixin and own fields
        self.mapped("mixin_field")
        #            ^complete concrete_field mixin_field mixin_value


class NoRulesModel(models.Model):
    """Model without access rules - should show diagnostic"""

    _name = "no.rules.model"
    #        ^diag Model `no.rules.model` has no access rules defined. Consider adding rules in security/ir.model.access.csv
    _inherit = "test.mixin"

    own_field = fields.Char()

    def test_fields(self):
        # Even without access rules, field completions should work
        self.mapped("own_field")
        #            ^complete mixin_field mixin_value own_field
