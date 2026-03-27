from odoo import http


def helper_returns_string():
    """A module-level helper function that returns a string."""
    return "hello"


def helper_returns_int():
    """A module-level helper function that returns an int."""
    return 42


class CustomController(http.Controller):
    def helper_method(self):
        return "helper"
