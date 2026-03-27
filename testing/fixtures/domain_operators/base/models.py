# Base models for domain operator tests


class ResPartner(Model):
    _name = "res.partner"

    name = fields.Char()
    country_id = fields.Many2one("res.country")
    child_ids = fields.One2many("res.partner", "parent_id")
    tag_ids = fields.Many2many("res.tag")
    parent_id = fields.Many2one("res.partner")
    age = fields.Integer()
    state = fields.Selection(
        [
            ("draft", "Draft"),
            ("confirmed", "Confirmed"),
            ("done", "Done"),
        ]
    )
    active = fields.Boolean()


class ResCountry(Model):
    _name = "res.country"

    code = fields.Char()
    name = fields.Char()


class ResTag(Model):
    _name = "res.tag"

    name = fields.Char()
    color = fields.Integer()
