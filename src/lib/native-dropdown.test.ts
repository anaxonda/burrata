// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { installNativeDropdowns } from "./native-dropdown";

let root: HTMLDivElement;
let remove: () => void;

beforeEach(() => {
  root = document.createElement("div");
  root.id = "app";
  root.className = "app-shell";
  document.body.append(root);
  remove = installNativeDropdowns(document.body);
});

afterEach(() => {
  remove();
  root.remove();
});

function addSelect() {
  root.innerHTML = `
    <form><label>Sort
      <select id="sort" name="sort">
        <option value="title">Title</option>
        <option value="artist" disabled>Artist</option>
        <option value="album">Album</option>
        <option value="date">Release date</option>
      </select></label>
      <button>Next</button>
    </form>`;
  return root.querySelector("select")!;
}

function press(select: HTMLSelectElement, key: string) {
  return select.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
}

function menu() {
  return root.querySelector<HTMLElement>(".native-dropdown-menu");
}

function options() {
  return [...root.querySelectorAll<HTMLElement>(".native-dropdown-option")];
}

describe("desktop single-select menus", () => {
  it("keeps native focus and submits the selected value with native input and change events", () => {
    const select = addSelect();
    const events: string[] = [];
    for (const name of ["input", "change"]) {
      root.addEventListener(name, event => {
        expect(event.target).toBe(select);
        events.push(`${event.type}:${select.value}`);
      });
    }

    select.click();
    expect(document.activeElement).toBe(select);
    expect(menu()?.getAttribute("aria-label")).toBe("Sort");
    expect(menu()?.id).toBe(select.getAttribute("aria-controls"));
    expect(options()[0].getAttribute("aria-selected")).toBe("true");
    options()[2].click();

    expect(select.value).toBe("album");
    expect(new FormData(select.form!).get("sort")).toBe("album");
    expect(events).toEqual(["input:album", "change:album"]);
    expect(document.activeElement).toBe(select);
    expect(menu()).toBeNull();
    expect(select.hasAttribute("aria-expanded")).toBe(false);

    select.click();
    options()[2].click();
    expect(events).toHaveLength(2);
  });

  it("navigates past disabled options and applies only the committed choice", () => {
    const select = addSelect();
    select.focus();
    expect(press(select, "ArrowDown")).toBe(false);
    expect(select.getAttribute("aria-activedescendant")).toBe(options()[0].id);
    expect(options()[1].getAttribute("aria-disabled")).toBe("true");
    options()[1].click();
    expect(menu()).toBeTruthy();

    press(select, "ArrowDown");
    expect(select.getAttribute("aria-activedescendant")).toBe(options()[2].id);
    expect(select.value).toBe("title");
    press(select, "End");
    press(select, "Escape");
    expect(select.value).toBe("title");
    expect(menu()).toBeNull();

    press(select, "r");
    expect(select.getAttribute("aria-activedescendant")).toBe(options()[3].id);
    expect(press(select, "Tab")).toBe(true);
    expect(select.value).toBe("date");
    expect(menu()).toBeNull();

    press(select, "Home");
    press(select, "Enter");
    expect(select.value).toBe("title");
  });

  it("reads live options, dismisses on removal or blur, and leaves unsupported selects native", async () => {
    const select = addSelect();
    select.click();
    select.options[2].remove();
    select.add(new Option("Duration", "duration"));
    await vi.waitFor(() => expect(options().map(option => option.textContent)).toEqual([
      "Title", "Artist", "Release date", "Duration",
    ]));
    options()[3].click();
    expect(select.value).toBe("duration");

    select.click();
    root.querySelector("button")!.focus();
    expect(menu()).toBeNull();

    select.disabled = true;
    expect(press(select, "ArrowDown")).toBe(true);
    expect(menu()).toBeNull();
    select.disabled = false;
    select.multiple = true;
    select.click();
    expect(menu()).toBeNull();
    select.multiple = false;

    select.click();
    select.remove();
    await vi.waitFor(() => expect(menu()).toBeNull());
  });

  it("keeps dropdown interaction inside a filter moved to the body", async () => {
    const select = addSelect();
    const filter = document.createElement("div");
    filter.className = "duration-inputs";
    filter.append(select);
    document.body.append(filter);
    try {
      select.click();
      expect(filter.querySelector('[role="listbox"]')).toBeTruthy();
      filter.querySelector<HTMLElement>('[role="option"]:last-child')!.click();
      expect(select.value).toBe("date");
      select.click();
      filter.hidden = true;
      await vi.waitFor(() => expect(filter.querySelector('[role="listbox"]')).toBeNull());
    } finally {
      filter.remove();
    }
  });
});
