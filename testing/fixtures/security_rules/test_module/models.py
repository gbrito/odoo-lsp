class SaleOrder(Model):
    _name = "sale.order"

    name = fields.Char()
    partner_id = fields.Many2one("res.partner")

    # Test groups validation - valid groups
    restricted_field = fields.Char(groups="base.group_user")
    #                                      ^def

    # Test groups validation - invalid group
    bad_field = fields.Char(groups="base.invalid_group")
    #                               ^diag No XML record with ID `base.invalid_group` found


class NoAccessRules(Model):
    _name = "no.access.rules"
    #        ^diag Model `no.access.rules` has no access rules defined. Consider adding rules in security/ir.model.access.csv

    name = fields.Char()
