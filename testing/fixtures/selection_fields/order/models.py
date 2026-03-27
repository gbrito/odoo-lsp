STATE_CHOICES = [
    ("draft", "Draft"),
    ("confirmed", "Confirmed"),
    ("done", "Done"),
]


class Order(Model):
    _name = "test.order"

    # Pattern 1: Variable reference for selection
    state = fields.Selection(STATE_CHOICES)

    # Pattern 2: Inline selection list as first positional argument
    status = fields.Selection(
        [
            ("pending", "Pending"),
            ("processing", "Processing"),
            ("complete", "Complete"),
        ]
    )

    # Pattern 3: selection= keyword argument
    priority = fields.Selection(
        selection=[
            ("low", "Low"),
            ("medium", "Medium"),
            ("high", "High"),
        ]
    )

    name = fields.Char()

    def test_state_completions(self):
        # Test completion in dict value for state field (variable reference)
        self.create({"state": ""})
        #                      ^complete confirmed done draft

    def test_status_completions(self):
        # Test completion in dict value for status field (inline list)
        self.create({"status": ""})
        #                       ^complete complete pending processing

    def test_priority_completions(self):
        # Test completion in dict value for priority field (selection= kwarg)
        self.create({"priority": ""})
        #                         ^complete high low medium
