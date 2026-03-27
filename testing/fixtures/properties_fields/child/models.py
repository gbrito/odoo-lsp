# Child module with Properties fields that reference container's PropertiesDefinition
class ChildModel(Model):
    _name = "child.model"

    name = fields.Char("Name")
    # Many2one to container model
    container_id = fields.Many2one("container.model")
    another_id = fields.Many2one("another.container")

    # Valid Properties field with definition
    props = fields.Properties(
        "Properties",
        definition="container_id.props_def",
        #           ^def
    )

    # Test completion for definition parameter - before the dot
    props2 = fields.Properties(
        "Properties 2",
        definition="",
        #           ^complete another_id container_id
    )

    # Test completion for definition parameter - after the dot
    props3 = fields.Properties(
        "Properties 3",
        definition="container_id.props_def",
        #                        ^complete props_def secondary_props_def
    )

    # Properties field missing definition parameter - should warn
    props_no_def = fields.Properties("No Definition")
    #                     ^diag Properties field should have a 'definition' parameter specifying the definition source

    # Invalid definition: non-existent m2o field
    props_bad_m2o = fields.Properties(
        "Bad M2O",
        definition="nonexistent_field.props_def",
        #           ^diag Field 'nonexistent_field' not found on model 'child.model'
    )

    # Invalid definition: m2o field that isn't Many2one
    props_not_m2o = fields.Properties(
        "Not M2O",
        definition="name.props_def",
        #           ^diag Field 'name' must be a Many2one field, but it's a Char field
    )

    # Invalid definition: non-existent propdef field on comodel
    props_bad_propdef = fields.Properties(
        "Bad PropDef",
        definition="container_id.nonexistent_propdef",
        #                        ^diag Field 'nonexistent_propdef' not found on model 'container.model'
    )

    # Invalid definition: propdef field that isn't PropertiesDefinition
    props_not_propdef = fields.Properties(
        "Not PropDef",
        definition="container_id.name",
        #                        ^diag Field 'name' must be a PropertiesDefinition field, but it's a Char field
    )

    # Invalid definition: missing dot
    props_no_dot = fields.Properties(
        "No Dot",
        definition="container_id",
        #           ^diag Invalid definition format 'container_id'. Expected: 'many2one_field.properties_definition_field'
    )

    # Invalid definition: multiple dots
    props_multi_dot = fields.Properties(
        "Multi Dot",
        definition="container_id.props_def.extra",
        #           ^diag Invalid definition format 'container_id.props_def.extra'. Expected exactly one dot: 'many2one_field.properties_definition_field'
    )

    # Valid with secondary container
    props_secondary = fields.Properties(
        "Secondary",
        definition="another_id.other_props_def",
    )

    def test_domain_with_properties(self):
        # Test that Properties fields in domains don't produce false errors
        # Note: The property name after the dot is dynamic and can't be validated
        self.search([("props.my_custom_property", "=", "value")])
        # Should NOT produce "props is not a relational field" error

        # Regular dotted access should still validate the first field
        self.search([("container_id.name", "=", "test")])
        #             ^complete another_id container_id name props props2 props3 props_bad_m2o props_bad_propdef props_multi_dot props_no_def props_no_dot props_not_m2o props_not_propdef props_secondary
