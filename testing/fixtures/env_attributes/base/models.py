class ResUsers(Model):
    _name = "res.users"

    name = fields.Char()
    login = fields.Char()
    partner_id = fields.Many2one("res.partner")


class ResCompany(Model):
    _name = "res.company"

    name = fields.Char()
    currency_id = fields.Many2one("res.currency")
