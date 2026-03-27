# ============================================================
# Test Fixture: ORM Method Type Preservation
# Tests that ORM methods correctly preserve/transform types
# ============================================================


class ResPartner(Model):
    _name = "res.partner"

    name = fields.Char()
    active = fields.Boolean()
    country_id = fields.Many2one("res.country")
    child_ids = fields.One2many("res.partner", "parent_id")
    parent_id = fields.Many2one("res.partner")


class ResCountry(Model):
    _name = "res.country"

    name = fields.Char()
    code = fields.Char()


class TestOrmMethods(Model):
    _name = "test.orm.methods"

    name = fields.Char()
    value = fields.Integer()
    partner_id = fields.Many2one("res.partner")
    line_ids = fields.One2many("test.line", "parent_id")

    # ============================================================
    # SECTION 1: Type-Preserving Methods
    # These methods return the same model type as the caller
    # ============================================================

    def test_filtered_preserves_type(self):
        # filtered() with lambda preserves model type
        filtered = self.filtered(lambda r: r.name)
        #^type Model["test.orm.methods"]

    def test_filtered_string_preserves_type(self):
        # filtered() with string field name
        filtered = self.filtered("name")
        #^type Model["test.orm.methods"]

    def test_sorted_preserves_type(self):
        # sorted() with lambda preserves model type
        sorted_recs = self.sorted(key=lambda r: r.name)
        #^type Model["test.orm.methods"]

    def test_sorted_string_preserves_type(self):
        # sorted() with string field name
        sorted_recs = self.sorted("name")
        #^type Model["test.orm.methods"]

    def test_sudo_preserves_type(self):
        # sudo() preserves model type
        sudo_recs = self.sudo()
        #^type Model["test.orm.methods"]

    def test_exists_preserves_type(self):
        # exists() preserves model type
        existing = self.exists()
        #^type Model["test.orm.methods"]

    def test_with_context_preserves_type(self):
        # with_context() preserves model type
        ctx_recs = self.with_context(active_test=False)
        #^type Model["test.orm.methods"]

    def test_with_user_preserves_type(self):
        # with_user() preserves model type
        user_recs = self.with_user(1)
        #^type Model["test.orm.methods"]

    def test_with_company_preserves_type(self):
        # with_company() preserves model type
        company_recs = self.with_company(1)
        #^type Model["test.orm.methods"]

    def test_with_env_preserves_type(self):
        # with_env() preserves model type
        env_recs = self.with_env(self.env)
        #^type Model["test.orm.methods"]

    # ============================================================
    # SECTION 2: Methods That Return Same Model
    # ============================================================

    def test_browse_returns_model(self):
        # browse() returns the model type
        records = self.browse([1, 2, 3])
        #^type Model["test.orm.methods"]

    def test_search_returns_model(self):
        # search() returns the model type
        found = self.search([("name", "=", "test")])
        #^type Model["test.orm.methods"]

    def test_create_returns_model(self):
        # create() returns the model type
        created = self.create({"name": "New"})
        #^type Model["test.orm.methods"]

    def test_copy_returns_model(self):
        # copy() returns the model type
        copied = self.copy()
        #^type Model["test.orm.methods"]

    # ============================================================
    # SECTION 3: env[] Access Returns Correct Model
    # ============================================================

    def test_env_model_access(self):
        # self.env[model] returns correct model type
        partners = self.env["res.partner"]
        #^type Model["res.partner"]

    def test_env_model_browse(self):
        # self.env[model].browse() returns correct model type
        partner = self.env["res.partner"].browse(1)
        #^type Model["res.partner"]

    def test_env_model_search(self):
        # self.env[model].search() returns correct model type
        partners = self.env["res.partner"].search([])
        #^type Model["res.partner"]

    # ============================================================
    # SECTION 4: Chained Method Calls
    # ============================================================

    def test_chained_sudo_filtered(self):
        # Chain: sudo() -> filtered()
        result = self.sudo().filtered(lambda r: r.name)
        #^type Model["test.orm.methods"]

    def test_chained_filtered_sorted(self):
        # Chain: filtered() -> sorted()
        result = self.filtered(lambda r: r.value > 0).sorted("name")
        #^type Model["test.orm.methods"]

    def test_chained_context_sudo_filtered(self):
        # Chain: with_context() -> sudo() -> filtered()
        result = self.with_context(test=True).sudo().filtered("name")
        #^type Model["test.orm.methods"]

    def test_chained_on_env_model(self):
        # Chain on self.env[model]
        result = self.env["res.partner"].sudo().filtered(lambda p: p.active)
        #^type Model["res.partner"]

    # ============================================================
    # SECTION 5: mapped() Type Transformations
    # ============================================================

    def test_mapped_char_returns_list(self):
        # mapped() on Char field returns list[str]
        names = self.mapped("name")
        #^type list[str]

    def test_mapped_integer_returns_list(self):
        # mapped() on Integer field returns list[int]
        values = self.mapped("value")
        #^type list[int]

    def test_mapped_m2o_returns_model(self):
        # mapped() on Many2one returns the related model
        partners = self.mapped("partner_id")
        #^type Model["res.partner"]

    def test_mapped_dotted_path(self):
        # mapped() with dotted path follows relations
        countries = self.mapped("partner_id.country_id")
        #^type Model["res.country"]

    def test_mapped_lambda_returns_type(self):
        # mapped() with lambda returns list of lambda result type
        result = self.mapped(lambda r: r.name)
        #^type list[str]

    # ============================================================
    # SECTION 6: grouped() Type Transformations
    # ============================================================

    def test_grouped_returns_dict(self):
        # grouped() returns dict[groupby_type, Model]
        grouped = self.grouped("name")
        #^type dict[str, Model["test.orm.methods"]]

    def test_grouped_items(self):
        # grouped().items() iteration
        for name, records in self.grouped("name").items():
            #^type str
            records
            #^type Model["test.orm.methods"]


class TestLine(Model):
    _name = "test.line"

    name = fields.Char()
    parent_id = fields.Many2one("test.orm.methods")
    quantity = fields.Float()

    def test_parent_access(self):
        # Access parent model through Many2one
        parent = self.parent_id
        #^type Model["test.orm.methods"]

        # Chain through parent
        parent_name = self.parent_id.name
        #^type str
