from odoo import Commands


class Cheese(models.Model):
    _name = 'cheese'

    name = fields.Char()


class CheeseBox(models.Model):
    _name = 'cheese.box'

    cheese_ids = fields.One2many('cheese')
    child_ids = fields.One2many('cheese.box')
    parent_id = fields.Many2one('cheese.box')

    def test_tuple_create(self):
        # Tuple CREATE: (0, 0, {values})
        self.create([{
            'cheese_ids': [(0, 0, {
                'name': ...
                #^complete name
            })],
            'child_ids': [(0, 0, {
                # ^complete child_ids
                'cheese_ids': [Command.create({
                    ''
                    #^complete name
                })]
            })]
        }])

    def test_tuple_update(self):
        # Tuple UPDATE: (1, id, {values})
        self.create([{
            'cheese_ids': [(1, 42, {
                'name': 'Updated'
                #^complete name
            })]
        }])

    def test_command_update(self):
        # Command.update() object form
        self.create([{
            'cheese_ids': [Command.update(42, {
                'name': 'Updated via Command'
                #^complete name
            })]
        }])
