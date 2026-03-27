# ============================================================
# Mixin B - Second parent for multiple inheritance tests
# ============================================================


class MixinB(Model):
    _name = "mixin.b"

    field_b = fields.Char()
    value_b = fields.Integer()

    def method_b(self):
        pass

    def _compute_b(self):
        pass
