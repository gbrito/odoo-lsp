class TestModel(Model):
    _name = "test.model"

    name = fields.Char()

    def test_env_user(self):
        # Test completion for self.env.
        user = self.env.user
        #              ^complete companies company context cr lang ref su uid user

        # Test completion for self.env.u
        u = self.env.u
        #            ^complete uid user

        # Test completion for self.env.c
        c = self.env.c
        #            ^complete companies company context cr

        # Test type for self.env.user
        user2 = self.env.user
        #                ^type Model["res.users"]

        # Test type for self.env.company
        company = self.env.company
        #                  ^type Model["res.company"]

        # Test type for self.env.companies
        companies = self.env.companies
        #                    ^type Model["res.company"]

        # Test type for self.env.uid
        uid = self.env.uid
        #              ^type int

        # Test type for self.env.lang
        lang = self.env.lang
        #               ^type None | str

        # Test type for self.env.su
        su = self.env.su
        #             ^type bool

        # Test definition for self.env.user (should jump to res.users model)
        user3 = self.env.user
        #                ^def

        # Test definition for self.env.company (should jump to res.company model)
        company2 = self.env.company
        #                   ^def
