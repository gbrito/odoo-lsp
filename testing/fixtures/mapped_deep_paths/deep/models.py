# ============================================================
# Test Fixture: Deep Mapped Paths and Magic Fields
# Tests for self.mapped() with deep paths and builtin fields
# ============================================================


# ============================================================
# SECTION 1: Base models for testing deep paths
# ============================================================


class Country(Model):
    _name = "test.country"

    name = fields.Char()
    code = fields.Char()


class Partner(Model):
    _name = "test.partner"

    name = fields.Char()
    country_id = fields.Many2one("test.country")


class Company(Model):
    _name = "test.company"

    name = fields.Char()
    partner_id = fields.Many2one("test.partner")


class Order(Model):
    _name = "test.order"

    name = fields.Char()
    company_id = fields.Many2one("test.company")
    partner_id = fields.Many2one("test.partner")

    # --------------------------------------------------------
    # SECTION 2: Deep mapped paths (2-3 hops)
    # --------------------------------------------------------

    def test_deep_paths(self):
        # Two-hop path: order -> partner -> name
        self.mapped("partner_id.name")
        #                       ^complete name country_id

        # Two-hop path: order -> partner -> country
        self.mapped("partner_id.country_id")
        #                       ^complete name country_id

        # Three-hop path: order -> partner -> country -> code
        self.mapped("partner_id.country_id.code")
        #                                  ^complete name code

        # Three-hop path: order -> partner -> country -> name
        self.mapped("partner_id.country_id.name")
        #                                  ^complete name code

        # Three-hop path via company: order -> company -> partner -> name
        self.mapped("company_id.partner_id.name")
        #                                  ^complete name country_id

        # Four-hop path: order -> company -> partner -> country -> code
        self.mapped("company_id.partner_id.country_id.code")
        #                                             ^complete name code

    # --------------------------------------------------------
    # SECTION 3: Magic/Builtin fields in mapped
    # These should NOT produce diagnostics (tested by bug fix)
    # --------------------------------------------------------

    def test_magic_fields(self):
        # 'ids' is a builtin field (bug fix verified this)
        self.mapped("ids")
        # No diagnostic - ids is in MAPPED_BUILTINS

        # 'id' is a builtin field
        self.mapped("id")
        # No diagnostic - id is in MAPPED_BUILTINS

        # 'display_name' is a builtin field
        self.mapped("display_name")
        # No diagnostic - display_name is in MAPPED_BUILTINS

        # create_date is a builtin field
        self.mapped("create_date")
        # No diagnostic - create_date is in MAPPED_BUILTINS

        # write_date is a builtin field
        self.mapped("write_date")
        # No diagnostic - write_date is in MAPPED_BUILTINS

        # create_uid is a builtin field
        self.mapped("create_uid")
        # No diagnostic - create_uid is in MAPPED_BUILTINS

        # write_uid is a builtin field
        self.mapped("write_uid")
        # No diagnostic - write_uid is in MAPPED_BUILTINS

    # --------------------------------------------------------
    # SECTION 4: Invalid deep paths (diagnostics)
    # --------------------------------------------------------

    def test_invalid_deep_paths(self):
        # Invalid field at second hop
        self.mapped("partner_id.nonexistent")
        #                       ^diag Model `test.partner` has no field `nonexistent`

        # Invalid field at third hop
        self.mapped("partner_id.country_id.bad_field")
        #                                  ^diag Model `test.country` has no field `bad_field`

        # Non-relational field used in path
        self.mapped("partner_id.name.something")
        #                       ^diag `name.something` is not a relational field

        # Multiple levels of error (first error stops)
        self.mapped("company_id.bad.also_bad")
        #                       ^diag `bad.also_bad` is not a relational field

    # --------------------------------------------------------
    # SECTION 5: Completions at different positions
    # --------------------------------------------------------

    def test_completions(self):
        # Completions on first field
        self.mapped("")
        #            ^complete name company_id partner_id

        # Completions on second field after Many2one
        self.mapped("partner_id.")
        #                       ^complete name country_id

        # Completions on third field
        self.mapped("partner_id.country_id.")
        #                                  ^complete name code
