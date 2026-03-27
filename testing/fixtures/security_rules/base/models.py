class ResPartner(Model):
    _name = "res.partner"

    name = fields.Char()


class ResUsers(Model):
    _name = "res.users"

    name = fields.Char()
    partner_id = fields.Many2one("res.partner")
