import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { mount } from "@vue/test-utils";
import { afterEach, describe, expect, it } from "vitest";

import CodexSurface from "../src/components/CodexSurface.vue";
import { installRequestTemplate } from "../src/runtime/request-template";

afterEach(() => {
  document.body.innerHTML = "";
});

describe("CodexSurface", () => {
  it("mounts the compatibility shell expected by the existing runtime", () => {
    const wrapper = mount(CodexSurface, { attachTo: document.body });

    expect(wrapper.find("#output").exists()).toBe(true);
    expect(wrapper.find("#messageInput").attributes("contenteditable")).toBe("true");
    expect(wrapper.find("#sessionPicker").exists()).toBe(true);
    expect(wrapper.find("#modelMenu").exists()).toBe(true);
    expect(wrapper.find("#permissionMenu").exists()).toBe(true);
    expect(wrapper.find("#requests").exists()).toBe(true);
    expect(wrapper.find("#controlModeSwitch").attributes("data-mode")).toBe("sync");
    const controlModes = wrapper.findAll("#controlModeSwitch [data-control-mode]");
    expect(controlModes).toHaveLength(2);
    expect(controlModes[0].attributes("aria-pressed")).toBe("true");
    expect(controlModes.every((button) => button.attributes("disabled") !== undefined)).toBe(true);

    wrapper.unmount();
  });

  it("keeps every compatibility element from the legacy shell", () => {
    mount(CodexSurface, { attachTo: document.body });
    installRequestTemplate();

    const legacyHtml = readFileSync(resolve(process.cwd(), "../public/index.html"), "utf8");
    const legacyDocument = new DOMParser().parseFromString(legacyHtml, "text/html");
    const expected = [...legacyDocument.querySelectorAll<HTMLElement>("[id]")]
      .map((element) => ({ id: element.id, tag: element.tagName, className: element.className }))
      .sort((left, right) => left.id.localeCompare(right.id));
    const actual = [...document.querySelectorAll<HTMLElement>("[id]")]
      .filter((element) => element.id !== "app")
      .map((element) => ({ id: element.id, tag: element.tagName, className: element.className }))
      .sort((left, right) => left.id.localeCompare(right.id));

    expect(actual).toEqual(expected);
  });
});
