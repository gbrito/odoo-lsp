# ============================================================
# Multiple Inheritance Tests
# Tests _inherit = ['model.a', 'model.b'] pattern
# ============================================================


class Combined(Model):
    _name = "combined"
    _inherit = ["mixin.a", "mixin.b"]

    own_field = fields.Char()

    # --------------------------------------------------------
    # COMPLETION TESTS - Should see fields from both parents
    # --------------------------------------------------------

    def test_completions(self):
        # mapped() completes fields from both parents, own fields, and descendants
        self.mapped("field_a")
        #            ^complete computed_a computed_bad extra_field field_a field_b own_field value_a value_b

    # --------------------------------------------------------
    # COMPUTE TESTS - Methods from either parent are valid
    # --------------------------------------------------------

    # Using method from mixin.a - should NOT produce diagnostic
    computed_a = fields.Char(compute="_compute_a")

    # Using non-existent method - SHOULD produce diagnostic
    computed_bad = fields.Char(compute="_nonexistent")
    #                                   ^diag Model `combined` has no method `_nonexistent`

    # --------------------------------------------------------
    # DIAGNOSTIC TESTS - Invalid field access
    # --------------------------------------------------------

    def test_diagnostics(self):
        self.mapped("bogus_field")
        #            ^diag Model `combined` has no field `bogus_field`


# ============================================================
# Extension of Combined Model
# ============================================================


class ExtendedCombined(Model):
    _inherit = "combined"

    extra_field = fields.Char()

    def test_extended_completions(self):
        # Should see all fields including own and inherited
        self.mapped("extra_field")
        #            ^complete computed_a computed_bad extra_field field_a field_b own_field value_a value_b


# ============================================================
# SECTION: Diamond Inheritance Pattern
# A inherits from both B and C, where both B and C inherit from D
# ============================================================


class DiamondBase(Model):
    """The common ancestor in diamond pattern"""

    _name = "diamond.base"

    base_field = fields.Char()
    base_value = fields.Integer()

    def base_method(self):
        pass


class DiamondLeft(Model):
    """Left branch of diamond - inherits from base"""

    _name = "diamond.left"
    _inherit = "diamond.base"

    left_field = fields.Char()

    def left_method(self):
        pass


class DiamondRight(Model):
    """Right branch of diamond - inherits from base"""

    _name = "diamond.right"
    _inherit = "diamond.base"

    right_field = fields.Char()

    def right_method(self):
        pass


class DiamondChild(Model):
    """Child that inherits from both branches (diamond pattern)"""

    _name = "diamond.child"
    _inherit = ["diamond.left", "diamond.right"]

    child_field = fields.Char()

    def test_diamond_all_fields(self):
        # Should have fields from entire diamond hierarchy
        # base_field from diamond.base (via both left and right)
        # left_field from diamond.left
        # right_field from diamond.right
        # child_field from self
        # computed_from_* are also fields on this model
        self.mapped("base_field")
        #            ^complete base_field base_value child_field computed_from_base computed_from_left computed_from_right computed_invalid left_field right_field

        self.mapped("left_field")
        #            ^complete base_field base_value child_field computed_from_base computed_from_left computed_from_right computed_invalid left_field right_field

        self.mapped("right_field")
        #            ^complete base_field base_value child_field computed_from_base computed_from_left computed_from_right computed_invalid left_field right_field

    # Test method resolution from diamond
    computed_from_base = fields.Char(compute="base_method")

    computed_from_left = fields.Char(compute="left_method")

    computed_from_right = fields.Char(compute="right_method")

    # Invalid method - should error
    computed_invalid = fields.Char(compute="_nonexistent_diamond")
    #                                       ^diag Model `diamond.child` has no method `_nonexistent_diamond`


# ============================================================
# SECTION: Deep Diamond (4 levels)
# Child -> [Left, Right] -> [LeftBase, RightBase] -> Root
# ============================================================


class DeepRoot(Model):
    _name = "deep.root"

    root_field = fields.Char()


class DeepLeftBase(Model):
    _name = "deep.left.base"
    _inherit = "deep.root"

    left_base_field = fields.Char()


class DeepRightBase(Model):
    _name = "deep.right.base"
    _inherit = "deep.root"

    right_base_field = fields.Char()


class DeepLeft(Model):
    _name = "deep.left"
    _inherit = "deep.left.base"

    left_field = fields.Char()


class DeepRight(Model):
    _name = "deep.right"
    _inherit = "deep.right.base"

    right_field = fields.Char()


class DeepChild(Model):
    """4-level deep diamond inheritance"""

    _name = "deep.child"
    _inherit = ["deep.left", "deep.right"]

    child_field = fields.Char()

    def test_deep_diamond_fields(self):
        # Should have fields from all 4 levels
        self.mapped("root_field")
        #            ^complete child_field left_base_field left_field right_base_field right_field root_field

        self.mapped("child_field")
        #            ^complete child_field left_base_field left_field right_base_field right_field root_field
