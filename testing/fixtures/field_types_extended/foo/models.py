# ============================================================
# Test Fixture for Extended Field Types
# Tests field types implemented but not covered in other fixtures:
# - Html, Text (String category)
# - Binary, Image (Binary category)
# - Date, Datetime (Date/Datetime category)
# - Monetary (Numeric category)
# - Json, Properties (Structured category)
# ============================================================


class AllFieldTypes(Model):
    _name = "all.field.types"

    # ===========================================
    # String-like fields
    # ===========================================

    char_field = fields.Char()
    text_field = fields.Text()
    html_field = fields.Html()

    # ===========================================
    # Binary fields
    # ===========================================

    binary_field = fields.Binary()
    image_field = fields.Image()

    # ===========================================
    # Date/Time fields
    # ===========================================

    date_field = fields.Date()
    datetime_field = fields.Datetime()

    # ===========================================
    # Numeric fields
    # ===========================================

    integer_field = fields.Integer()
    float_field = fields.Float()
    monetary_field = fields.Monetary()
    currency_id = fields.Many2one("res.currency")

    # ===========================================
    # Structured fields
    # ===========================================

    json_field = fields.Json()
    properties_field = fields.Properties()
    #                         ^diag Properties field should have a 'definition' parameter specifying the definition source

    # ===========================================
    # Relational fields
    # ===========================================

    partner_id = fields.Many2one("res.partner")
    tag_ids = fields.Many2many("res.tag")
    line_ids = fields.One2many("line.model", "parent_id")

    def test_all_field_completions(self):
        # All fields should appear in completions
        self.mapped("")
        #            ^complete binary_field char_field currency_id date_field datetime_field float_field html_field image_field integer_field json_field line_ids monetary_field partner_id properties_field tag_ids text_field


class ResPartner(Model):
    _name = "res.partner"

    name = fields.Char()
    html_content = fields.Html()
    binary_data = fields.Binary()
    image_data = fields.Image()
    birth_date = fields.Date()
    last_login = fields.Datetime()
    balance = fields.Monetary()
    currency_id = fields.Many2one("res.currency")
    metadata = fields.Json()
    dynamic_props = fields.Properties()
    #                      ^diag Properties field should have a 'definition' parameter specifying the definition source


class ResCurrency(Model):
    _name = "res.currency"

    name = fields.Char()
    symbol = fields.Char()


class ResTag(Model):
    _name = "res.tag"

    name = fields.Char()


class LineModel(Model):
    _name = "line.model"

    name = fields.Char()
    parent_id = fields.Many2one("all.field.types")


# ============================================================
# Domain Operator Tests for Extended Field Types
# ============================================================


class DomainOperatorTests(Model):
    _name = "domain.operator.tests"

    partner_id = fields.Many2one("res.partner")

    # ===========================================
    # Html field operators (String category)
    # All string operators valid
    # ===========================================

    html_ilike = fields.Many2one(
        "res.partner",
        domain=[("html_content", "ilike", "<p>test</p>")],
    )

    html_equals = fields.Many2one(
        "res.partner",
        domain=[("html_content", "=", "")],
    )

    # ===========================================
    # Binary/Image field operators (Binary category)
    # Only = and != are valid
    # ===========================================

    binary_equals = fields.Many2one(
        "res.partner",
        domain=[("binary_data", "=", False)],
    )

    binary_not_equals = fields.Many2one(
        "res.partner",
        domain=[("binary_data", "!=", False)],
    )

    image_equals = fields.Many2one(
        "res.partner",
        domain=[("image_data", "=", False)],
    )

    # ===========================================
    # Date field operators (Date category)
    # Comparison operators valid
    # ===========================================

    date_equals = fields.Many2one(
        "res.partner",
        domain=[("birth_date", "=", "2024-01-01")],
    )

    date_greater = fields.Many2one(
        "res.partner",
        domain=[("birth_date", ">=", "2024-01-01")],
    )

    date_less = fields.Many2one(
        "res.partner",
        domain=[("birth_date", "<", "2024-12-31")],
    )

    # ===========================================
    # Datetime field operators (Datetime category)
    # Comparison operators valid
    # ===========================================

    datetime_equals = fields.Many2one(
        "res.partner",
        domain=[("last_login", "=", "2024-01-01 00:00:00")],
    )

    datetime_greater = fields.Many2one(
        "res.partner",
        domain=[("last_login", ">=", "2024-01-01 00:00:00")],
    )

    # ===========================================
    # Monetary field operators (Numeric category)
    # All numeric operators valid
    # ===========================================

    monetary_equals = fields.Many2one(
        "res.partner",
        domain=[("balance", "=", 0)],
    )

    monetary_greater = fields.Many2one(
        "res.partner",
        domain=[("balance", ">", 100.0)],
    )

    monetary_in = fields.Many2one(
        "res.partner",
        domain=[("balance", "in", [0, 100, 1000])],
    )

    # ===========================================
    # Json/Properties field operators (Structured category)
    # Only = and != are valid
    # ===========================================

    json_equals = fields.Many2one(
        "res.partner",
        domain=[("metadata", "=", False)],
    )

    json_not_equals = fields.Many2one(
        "res.partner",
        domain=[("metadata", "!=", {})],
    )

    properties_equals = fields.Many2one(
        "res.partner",
        domain=[("dynamic_props", "=", False)],
    )
