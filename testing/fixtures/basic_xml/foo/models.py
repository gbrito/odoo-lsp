class Foo(Model):
    _name = "foo"

    bar = fields.Char(groups="bar.group_name")
    #                              ^complete bar.group_name
    baz = fields.Char(groups="bar.group_name,foo.g")
    #                                             ^complete foo.group_name
    unscoped = fields.Char(groups="g")
    #                               ^complete bar.group_name group_name

    def action_button(self):
        ...

    def action_button2(self):
        ...
