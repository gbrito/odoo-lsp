from odoo import models, fields

# ^symbol Bread
# ^symbol Wine
# ^symbol _test
# ^symbol identity


class Bread(models.Model):
    _name = "bakery.bread"
    #       ^token CLASS

    def _test(self):
        items = {item: item for item in self}
        # ^type dict[Model["bakery.bread"], Model["bakery.bread"]]

        foobar = {"a": self, "b": 123}

        aaaa = foobar["a"]
        # ^type Model["bakery.bread"]

        bbbb = foobar["b"]
        # ^type int

        return foobar

    def identity(self, what):
        return {"c": what}

    def _test_return(self):
        foobar = self._test()
        aaaa = foobar["a"]
        # ^type Model["bakery.bread"]
        bbbb = foobar["b"]
        # ^type int

        baz = self.identity(self)
        cccc = baz["c"]
        # ^type Model["bakery.bread"]

    def test_variable_append(self):
        foo = []
        # ^type list
        for _ in range(123):
            foo.append(self)
        foo
        # ^type list[Model["bakery.bread"]]

    def test_dictkey_append(self):
        foo = self.identity([])
        foo["c"].append(self)
        cccc = foo["c"]
        # ^type list[Model["bakery.bread"]]
        elem = cccc[12]
        # ^type Model["bakery.bread"]

    def test_dict_set(self):
        foobar = {}
        foobar["a"] = self
        aaaa = foobar["a"]
        # ^type Model["bakery.bread"]
        foobar["b"] = nonexistent

    def test_dict_update(self):
        foobar = {}
        foobar.update({"a": self})
        aaaa = foobar["a"]
        # ^type Model["bakery.bread"]

    def test_sanity(self):
        foobar = ["what"]
        # ^type list[str]

    def test_builtins(self):
        for aaaa, bbbb in enumerate(self):
            aaaa
            # ^type int
            bbbb
            # ^type Model["bakery.bread"]

        ints = [1, 2, 3]
        for aaaa, bbbb in zip(self, ints):
            aaaa
            # ^type Model["bakery.bread"]
            bbbb
            # ^type int

    def _identity_tuple(self, obj):
        return self, obj

    def _test_tuple(self):
        foo, bar = self._identity_tuple(123)
        # ^type Model["bakery.bread"]
        bar
        # ^type int

    def test_subscript(self):
        foobar = {"abcde": 123, "fool": 234}
        foobar[""]
        #      ^complete abcde fool
        foobar["f"]
        #        ^complete fool


class Wine(models.Model):
    _name = "bakery.wine"
    #       ^token CLASS

    name = fields.Char()
    #             ^token TYPE
    make = fields.Char()
    #             ^token TYPE
    value = fields.Float()
    #              ^token TYPE

    def _test_read_group(self):
        domain = []
        for name, make, value in self._read_group(domain, ['name', 'make'], ['value:sum']):
            #^type str
            make
            # ^type str
            value
            # ^type float

    def _test_mapped(self):
        foo = self.mapped("make")
        # ^type list[str]

    def test_grouped(self):
        for name, records in self.grouped("name").items():
            # ^type str
            records
            # ^type Model["bakery.wine"]
