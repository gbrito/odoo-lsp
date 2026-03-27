class PartnerExtension(Model):
    _inherit = "res.partner"

    # Case 1: Override without calling parent - should warn
    def action_confirm(self):
#   ^diag Method `action_confirm` overrides parent but does not call `super().action_confirm()`
#   ^related Parent method defined in `res.partner`
        """Custom implementation without calling parent."""
        return False

    # Case 2: Override with parent call - should NOT warn
    def compute_display_name(self):
        """Custom implementation with parent call."""
        result = super().compute_display_name()
        return f"[Extended] {result}"

    # Case 3: Override with parent using different syntax - should NOT warn
    def write(self, vals):
        """Override using ClassName.method(self) pattern."""
        result = super(PartnerExtension, self).write(vals)
        return result

    # Case 4: New method not in parent - should NOT warn
    def custom_method(self):
        """This method doesn't exist in parent, no parent call needed."""
        return "custom"

    # Case 5: Override private method without parent call - should warn
    def private_method(self):
#   ^diag Method `private_method` overrides parent but does not call `super().private_method()`
#   ^related Parent method defined in `res.partner`
        """Override private method without calling parent."""
        return "overridden"
