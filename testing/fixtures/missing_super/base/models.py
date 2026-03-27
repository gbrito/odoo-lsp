class Partner(Model):
    _name = "res.partner"

    def action_confirm(self):
        """Confirm the partner."""
        return True

    def compute_display_name(self):
        """Compute display name."""
        return self.name

    def write(self, vals):
        """Override write."""
        return True

    def private_method(self):
        """A private method that might be intentionally not called."""
        pass
