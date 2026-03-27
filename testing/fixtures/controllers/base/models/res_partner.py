class ResPartner(Model):
    _name = "res.partner"

    name = fields.Char()
    email = fields.Char()
    active = fields.Boolean()
