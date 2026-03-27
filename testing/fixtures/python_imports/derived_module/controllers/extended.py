from odoo.addons.base_module.controllers.main import CustomController
#                                                    ^def

from odoo.addons.base_module.controllers.main import CustomController as MyBaseController
#                                                                        ^def

from odoo.addons.base_module.controllers.main import helper_returns_string
#                                                    ^def

from odoo.addons.base_module.controllers.main import helper_returns_int
#                                                    ^def

from odoo.addons.base_module.controllers.main import helper_returns_string as get_string
#                                                                             ^def

from odoo.addons.nonexistent_module.controllers.main import MissingController
#                                                           ^diag Cannot resolve import 'MissingController'

from odoo.addons.base_module.controllers.missing import AnotherMissingController
#                                                       ^diag Cannot resolve import 'AnotherMissingController'


class ExtendedController(CustomController):
    #                    ^def

    def test_imported_function_returns_string(self):
        result = helper_returns_string()
        # ^type str

    def test_imported_function_returns_int(self):
        result = helper_returns_int()
        # ^type int

    def test_aliased_imported_function(self):
        result = get_string()
        # ^type str


class ExtendedControllerWithBase(MyBaseController):
    #                            ^def
    ...
