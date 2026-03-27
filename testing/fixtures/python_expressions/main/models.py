from odoo import models


def helper_returns_model(env):
    """Helper function that returns a model."""
    return env["main"]


def helper_returns_string():
    """Helper function that returns a string."""
    return "hello"


def helper_returns_int():
    """Helper function that returns an int."""
    return 42


def helper_returns_union(flag):
    """Helper function that returns a union type."""
    if flag:
        return "hello"
    else:
        return 42


def helper_multiple_returns(env, mode):
    """Helper function with multiple return statements."""
    if mode == "model":
        return env["main"]
    elif mode == "string":
        return "result"
    return None


class Main(models.Model):
    _name = "main"

    def test_conditional_same_type(self):
        foo = self if True else self
        # ^type Model["main"]

    def test_conditional_union(self):
        bar = self if True else 123
        # ^type Model["main"] | int

    def test_conditional_str_int(self):
        baz = "hello" if True else 42
        # ^type int | str

    def test_boolean_or_union(self):
        foo = self or False
        # ^type Model["main"] | bool

    def test_boolean_and_union(self):
        foo = True and self
        # ^type Model["main"] | bool

    def test_except_single_type(self):
        try:
            x = 1
        except ValueError as e:
            e
          # ^type ValueError

    def test_except_tuple_type(self):
        try:
            x = 1
        except (TypeError, KeyError) as err:
            err
          # ^type KeyError | TypeError

    def test_function_returns_string(self):
        result = helper_returns_string()
        # ^type str

    def test_function_returns_int(self):
        result = helper_returns_int()
        # ^type int

    def test_function_returns_union(self):
        result = helper_returns_union(True)
        # ^type int | str

    def test_function_multiple_returns(self):
        result = helper_multiple_returns(self.env, "model")
        # ^type None | str
