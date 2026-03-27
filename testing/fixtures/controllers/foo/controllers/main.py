# Model for testing within this module
class Foo(Model):
    _name = "foo"

    some_field = fields.Char()


# ============================================================
# SECTION 1: Valid Routes (should NOT produce any diagnostics)
# ============================================================


class ValidController(http.Controller):
    # Valid HTTP route with all defaults
    @http.route("/valid/simple")
    def simple_route(self):
        pass

    # Valid route with explicit type='http'
    @http.route("/valid/http", type="http")
    def http_route(self):
        pass

    # Valid route with type='json'
    @http.route("/valid/json", type="json")
    def json_route(self):
        pass

    # Valid auth types
    @http.route("/valid/auth/user", auth="user")
    def auth_user(self):
        pass

    @http.route("/valid/auth/public", auth="public")
    def auth_public(self):
        pass

    @http.route("/valid/auth/none", auth="none")
    def auth_none(self):
        pass

    @http.route("/valid/auth/bearer", auth="bearer")
    def auth_bearer(self):
        pass

    # Valid URL parameters with matching method signature
    @http.route("/valid/param/<int:order_id>")
    def with_int_param(self, order_id):
        pass

    # Valid string parameter
    @http.route("/valid/param/<name>")
    def with_string_param(self, name):
        pass

    # Valid model converter with existing model (foo is defined in this module)
    @http.route('/valid/foo/<model("foo"):item>')
    def with_model_param(self, item):
        pass

    # Multiple paths (list syntax)
    @http.route(["/valid/multi/path1", "/valid/multi/path2"])
    def multi_path_route(self):
        pass

    # Route with **kwargs accepts any URL params
    @http.route("/valid/kwargs/<int:anything>")
    def with_kwargs(self, **kw):
        pass

    # Route with *args accepts any URL params
    @http.route("/valid/args/<int:something>")
    def with_args(self, *args):
        pass


# ============================================================
# SECTION 2: Invalid Route Type (ERROR diagnostics)
# ============================================================


class InvalidTypeController(http.Controller):
    @http.route("/invalid/type", type="invalid")
    #                                 ^diag Invalid route type 'invalid'. Valid types: http, json
    def invalid_type_route(self):
        pass

    @http.route("/invalid/type2", type="rpc")
    #                                  ^diag Invalid route type 'rpc'. Valid types: http, json
    def invalid_type_rpc(self):
        pass


# ============================================================
# SECTION 3: Invalid Auth Type (ERROR diagnostics)
# ============================================================


class InvalidAuthController(http.Controller):
    @http.route("/invalid/auth", auth="bogus")
    #                                 ^diag Invalid auth type 'bogus'. Valid types: user, public, none, bearer
    def invalid_auth_route(self):
        pass

    @http.route("/invalid/auth2", auth="admin")
    #                                  ^diag Invalid auth type 'admin'. Valid types: user, public, none, bearer
    def invalid_auth_admin(self):
        pass


# ============================================================
# SECTION 4: Duplicate Routes (WARNING diagnostics)
# ============================================================


class DuplicateController(http.Controller):
    @http.route("/duplicate/path")
    def first_handler(self):
        pass

    @http.route("/duplicate/path")
    #           ^diag Duplicate route '/duplicate/path' in this module
    def second_handler(self):
        pass


# ============================================================
# SECTION 5: URL Parameter Not in Method Signature (WARNING)
# The diagnostic is on the method NAME (character 8 where the name starts)
# ============================================================


class MissingParamController(http.Controller):
    @http.route("/missing/<int:user_id>")
    def missing_param(self):
       #^diag URL parameter 'user_id' is not in method signature
        pass

    @http.route("/missing2/<name>/<int:id>")
    def missing_multiple(self, name):
       #^diag URL parameter 'id' is not in method signature
        pass


# ============================================================
# SECTION 6: Unknown Model in Route Converter (ERROR diagnostics)
# ============================================================


class UnknownModelController(http.Controller):
    @http.route('/unknown/<model("unknown.model"):obj>')
    #           ^diag Unknown model 'unknown.model' in route converter
    def unknown_model_route(self, obj):
        pass

    @http.route('/unknown2/<model("nonexistent"):item>')
    #           ^diag Unknown model 'nonexistent' in route converter
    def another_unknown(self, item):
        pass

    # Valid model should NOT produce diagnostic
    @http.route('/known/<model("res.partner"):partner>')
    def known_model_route(self, partner):
        pass


# ============================================================
# SECTION 7: Type inference in controllers (existing test)
# ============================================================


class TypeInferenceController(http.Controller):
    @http.route("/type/test")
    def type_test(self):
        foo = request.env["foo"]
        # ^type Model["foo"]
        foo.some_field
        #  ^complete some_field
