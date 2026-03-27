# Container module with PropertiesDefinition fields
class ContainerModel(Model):
    _name = "container.model"

    name = fields.Char("Name")
    props_def = fields.PropertiesDefinition("Properties Definition")
    secondary_props_def = fields.PropertiesDefinition("Secondary Properties")


class AnotherContainer(Model):
    _name = "another.container"

    name = fields.Char("Name")
    other_props_def = fields.PropertiesDefinition("Other Properties")
