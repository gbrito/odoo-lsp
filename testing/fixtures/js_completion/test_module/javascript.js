/** @odoo-module **/

import { Component, useState, useRef, onMounted } from "@odoo/owl";
import { useService, useBus } from "@web/core/utils/hooks";

export class TestWidget extends Component {
	static template = "test_module.TestWidget";

	setup() {
		// Test service completion - get all services with empty prefix
		this.orm = useService("");
		//                    ^complete action command company currency datetime_picker dialog effect error field file_upload hotkey http localization menu my_custom name notification orm overlay popover router rpc sortable title tooltip ui user view

		// Test useRef (valid ref name, no diagnostic)
		this.inputRef = useRef("input");

		// Test lifecycle hooks
		onMounted(() => {
			console.log("mounted");
		});

		// Test useState
		this.state = useState({ count: 0 });
	}

	async onButtonClick() {
		const result = await this.orm.call("test", "test_method");
		//                                  ^complete test.model

		// Test method name completion - list all methods
		const result2 = await this.orm.call("test.model", "");
		//                                                ^complete another_method test_method
	}
}

// Test custom service registration - this service should be discovered
const myCustomService = {
	dependencies: ["orm", "notification"],
	async: ["fetchData", "saveData"],
	start(env, { orm, notification }) {
		return {
			fetchData() {
				return orm.call("res.partner", "search_read");
			},
			saveData() {
				notification.add("Saved!");
			},
		};
	},
};

registry.category("services").add("my_custom", myCustomService);
registry.category("fields").add("foo").add("bar").add("baz");

// Test component that uses our custom service
export class CustomServiceUser extends Component {
	static template = "test_module.CustomServiceUser";

	setup() {
		// Test go-to-definition for a known service
		this.customService = useService("my_custom");
		//                               ^def
	}
}
