# ============================================================
# Mixin A - First parent for multiple inheritance tests
# ============================================================


class MixinA(Model):
    _name = "mixin.a"

    field_a = fields.Char()
    value_a = fields.Integer()

    def method_a(self):
        pass

    def _compute_a(self):
        pass
