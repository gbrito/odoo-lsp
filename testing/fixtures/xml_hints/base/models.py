class ResCompany(Model):
    _name = "res.company"

    name = fields.Char()


class ResPartner(Model):
    _name = "res.partner"

    name = fields.Char()
    email = fields.Char()
    company_id = fields.Many2one("res.company")
