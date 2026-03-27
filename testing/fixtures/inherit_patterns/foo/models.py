# ============================================================
# Test Fixture for Inheritance Patterns
# Tests both _inherit (list/string) and _inherits (dict)
# ============================================================


# ============================================================
# SECTION 1: Base Model
# ============================================================


class Foo(Model):
    _name = "foo"

    age = fields.Char()


# ============================================================
# SECTION 2: List Inherit Without _name (Issue #39)
# When _inherit = [models] without _name, inherits from first model
# ============================================================


class Bar(Model):
    """Validation for #39 - array inherit without _name"""

    # _name = 'foo'     # verifies that it still works without this
    _inherit = ["foo"]

    bar = fields.Char(related="age")
    #                          ^complete age bar extra_foo_field

    def main(self):
        self.mapped("age")
        #            ^complete age bar extra_foo_field
        self.mapped("aged")
        #            ^diag Model `foo` has no field `aged`


# ============================================================
# SECTION 3: Delegation Inheritance (_inherits)
# _inherits = {'parent.model': 'parent_field_id'} delegates to parent
# Fields from parent model are accessible on child
# ============================================================


class ParentModel(Model):
    _name = "parent.model"

    parent_name = fields.Char()
    parent_value = fields.Integer()


class ChildWithInherits(Model):
    """Test _inherits (dict) - delegation inheritance"""

    _name = "child.inherits"
    _inherits = {"parent.model": "parent_id"}

    parent_id = fields.Many2one("parent.model", required=True)
    child_field = fields.Char()

    def test_delegation(self):
        #                ^type Model["child.inherits"]

        # Should have access to own fields
        self.mapped("child_field")
        #            ^complete child_field parent_id parent_name parent_value

        # Should have access to delegated parent fields
        self.mapped("parent_name")
        #            ^complete child_field parent_id parent_name parent_value


# ============================================================
# SECTION 4: Combined Patterns
# Model with both _inherit and _inherits
# ============================================================


class MixinModel(Model):
    _name = "mixin.model"

    mixin_field = fields.Char()


class CombinedInheritance(Model):
    """Model using both _inherit and _inherits"""

    _name = "combined.inheritance"
    _inherit = "mixin.model"
    _inherits = {"parent.model": "parent_id"}

    parent_id = fields.Many2one("parent.model", required=True)
    own_field = fields.Char()

    def test_all_fields(self):
        # Should have mixin fields (from _inherit)
        # Should have parent fields (from _inherits delegation)
        # Should have own fields
        self.mapped("mixin_field")
        #            ^complete mixin_field own_field parent_id parent_name parent_value


# ============================================================
# SECTION 5: Multiple _inherits (Delegation to Multiple Parents)
# This is rare but valid in Odoo - delegates to multiple models
# ============================================================


class SecondParent(Model):
    _name = "second.parent"

    second_name = fields.Char()
    second_value = fields.Float()


class MultipleInherits(Model):
    """Test multiple _inherits - delegation to multiple parents"""

    _name = "multiple.inherits"
    _inherits = {
        "parent.model": "parent_id",
        "second.parent": "second_id",
    }

    parent_id = fields.Many2one("parent.model", required=True)
    second_id = fields.Many2one("second.parent", required=True)
    own_multi_field = fields.Char()

    def test_multiple_delegation(self):
        # Should have fields from both delegated parents
        self.mapped("parent_name")
        #            ^complete own_multi_field parent_id parent_name parent_value second_id second_name second_value

        self.mapped("second_name")
        #            ^complete own_multi_field parent_id parent_name parent_value second_id second_name second_value


# ============================================================
# SECTION 6: _inherit List Without _name (Multiple Parents)
# Extends multiple models, takes name from first
# ============================================================


class FirstBase(Model):
    _name = "first.base"

    first_field = fields.Char()


class SecondBase(Model):
    _name = "second.base"

    second_field = fields.Char()


class MultipleInheritNoName(Model):
    """Test _inherit list without _name - inherits from first model"""

    _inherit = ["first.base", "second.base"]

    added_field = fields.Char()

    def test_multiple_inherit_no_name(self):
        # Should have fields from both inherited models
        self.mapped("first_field")
        #            ^complete added_field first_field second_field


# ============================================================
# SECTION 7: Extension of Model (same _name)
# _inherit without _name but model already exists
# ============================================================


class FooExtension(Model):
    """Extension adds fields to existing 'foo' model"""

    _inherit = "foo"

    extra_foo_field = fields.Char()

    def test_extended_foo(self):
        # Should have original fields + extension fields
        self.mapped("age")
        #            ^complete age bar extra_foo_field


# ============================================================
# SECTION 8: Chain of Inheritance
# A inherits B inherits C
# ============================================================


class ChainBase(Model):
    _name = "chain.base"

    base_field = fields.Char()


class ChainMiddle(Model):
    _name = "chain.middle"
    _inherit = "chain.base"

    middle_field = fields.Char()


class ChainEnd(Model):
    _name = "chain.end"
    _inherit = "chain.middle"

    end_field = fields.Char()

    def test_chain_inheritance(self):
        # Should have fields from entire inheritance chain
        self.mapped("base_field")
        #            ^complete base_field end_field middle_field

        self.mapped("middle_field")
        #            ^complete base_field end_field middle_field

        self.mapped("end_field")
        #            ^complete base_field end_field middle_field
