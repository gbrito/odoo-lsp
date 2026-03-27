class Foo(Model):
    _name = "foo"

    bar = fields.Char(groups="bar.group_name")
    #                              ^complete bar.group_name
    baz = fields.Char(groups="bar.group_name,foo.g")
    #                                        ^diag No XML record with ID `foo.g` found
    unscoped = fields.Char(groups="g")
    #                              ^diag No XML record with ID `g` found

    def action_button(self): ...

    def action_button2(self): ...
