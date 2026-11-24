import { positionContextMenu } from "./menu-geometry";

const selectSelector = 'select:not([multiple]):not([size]:not([size="1"])):not([data-native-picker])';
const typeaheadDelayMs = 700;
const menuGap = 6;
let nextMenuId = 0;

type OpenDropdown = {
  select: HTMLSelectElement;
  menu: HTMLDivElement;
  options: HTMLOptionElement[];
  rows: HTMLDivElement[];
  active: number;
  query: string;
  lastTyped: number;
  observer: MutationObserver;
};

// Keep the native select as the focus, value, event, and form control. The menu
// is rendered only while open, including for selects mounted after startup.
export function installNativeDropdowns(root: HTMLElement): () => void {
  const document = root.ownerDocument;
  const window = document.defaultView;
  if (!window) return () => {};

  let open: OpenDropdown | null = null;
  const forcedColors = window.matchMedia?.("(forced-colors: active)");
  const listeners = new AbortController();
  const { signal } = listeners;
  root.setAttribute("data-burrata-selects", "");

  function dropdownTarget(target: EventTarget | null) {
    if (!(target instanceof HTMLSelectElement) || !root.contains(target)) return null;
    if (forcedColors?.matches || !target.matches(selectSelector) || target.matches(":disabled")) return null;
    if (target.closest('[inert], [hidden], dialog:not([open])')) return null;
    // The existing app only uses flat lists. New grouped or empty selects
    // keep their native picker until this UI supports their semantics.
    if (!target.options.length || target.querySelector("optgroup")) return null;
    return target;
  }

  function close() {
    if (!open) return;
    const { select, menu, observer } = open;
    open = null;
    observer.disconnect();
    menu.remove();
    select.removeAttribute("aria-expanded");
    select.removeAttribute("aria-controls");
    select.removeAttribute("aria-activedescendant");
  }

  function setActive(dropdown: OpenDropdown, index: number) {
    const { select, rows } = dropdown;
    rows[dropdown.active]?.removeAttribute("data-active");
    dropdown.active = index;
    const row = rows[index];
    if (!row) {
      select.removeAttribute("aria-activedescendant");
      return;
    }
    row.setAttribute("data-active", "");
    select.setAttribute("aria-activedescendant", row.id);
    row.scrollIntoView?.({ block: "nearest" });
  }

  function renderOptions(dropdown: OpenDropdown) {
    const { select, menu } = dropdown;
    const previouslyActive = dropdown.options[dropdown.active];
    dropdown.options = Array.from(select.options);
    dropdown.rows = dropdown.options.map((option, index) => {
      const row = document.createElement("div");
      row.id = `${menu.id}-${index}`;
      row.className = "dropdown-option native-dropdown-option";
      row.setAttribute("role", "option");
      row.setAttribute("aria-selected", String(index === select.selectedIndex));
      if (option.disabled) row.setAttribute("aria-disabled", "true");

      const check = document.createElement("span");
      check.className = "dropdown-check";
      check.setAttribute("aria-hidden", "true");
      const label = document.createElement("span");
      label.textContent = option.label;
      row.append(check, label);
      return row;
    });
    menu.replaceChildren(...dropdown.rows);
    const previousIndex = previouslyActive ? dropdown.options.indexOf(previouslyActive) : -1;
    const preferred = previousIndex >= 0 && !dropdown.options[previousIndex].disabled
      ? previousIndex : select.selectedIndex;
    const active = preferred >= 0 && !dropdown.options[preferred].disabled
      ? preferred
      : dropdown.options.findIndex(option => !option.disabled);
    const anchor = select.getBoundingClientRect();
    const viewport = { width: window!.innerWidth, height: window!.innerHeight };
    const availableHeight = Math.max(
      44, anchor.top - 2 * menuGap, viewport.height - anchor.bottom - 2 * menuGap,
    );
    const menuWidth = Math.min(Math.max(148, anchor.width), 280, viewport.width - 2 * menuGap);
    menu.style.minWidth = `${Math.max(0, menuWidth)}px`;
    menu.style.maxHeight = `min(18rem, ${availableHeight}px)`;
    const position = positionContextMenu(
      anchor, menu.getBoundingClientRect(), viewport, 0, menuGap, "start",
    );
    menu.style.left = `${position.left}px`;
    menu.style.top = `${position.top}px`;
    setActive(dropdown, active);
  }

  function commit(dropdown: OpenDropdown, index: number) {
    const { select, options } = dropdown;
    const option = options[index];
    close();
    if (!dropdownTarget(select) || !option || option.disabled) return;
    const currentIndex = Array.prototype.indexOf.call(select.options, option);
    if (currentIndex < 0 || select.selectedIndex === currentIndex) return;
    select.selectedIndex = currentIndex;
    select.dispatchEvent(new Event("input", { bubbles: true }));
    select.dispatchEvent(new Event("change", { bubbles: true }));
  }

  function show(select: HTMLSelectElement) {
    close();
    const menu = document.createElement("div");
    menu.id = `burrata-select-menu-${++nextMenuId}`;
    menu.className = "dropdown-menu native-dropdown-menu";
    menu.setAttribute("role", "listbox");
    const label = select.getAttribute("aria-label") ?? Array.from(select.labels ?? [], element => {
      const content = element.cloneNode(true) as HTMLElement;
      content.querySelectorAll("select, input, textarea, button, [aria-hidden=true]").forEach(control => control.remove());
      return content.textContent?.trim();
    }).filter(Boolean).join(" ");
    const labelledBy = select.getAttribute("aria-labelledby");
    if (labelledBy) menu.setAttribute("aria-labelledby", labelledBy);
    else if (label) menu.setAttribute("aria-label", label);

    // A dialog-local parent keeps the popup available inside a modal. A manual
    // popover moves it above clipped scroll panes on webviews that support it.
    const parent = select.closest("dialog, [popover], .duration-inputs, .app-shell") ?? root;
    const canUseTopLayer = typeof menu.showPopover === "function";
    if (canUseTopLayer) menu.setAttribute("popover", "manual");
    parent.append(menu);
    if (canUseTopLayer) {
      try {
        menu.showPopover();
      } catch {
        menu.removeAttribute("popover");
      }
    }

    const observer = new MutationObserver(() => {
      if (!open || open.select !== select) return;
      if (!dropdownTarget(select)) close();
      else renderOptions(open);
    });
    const dropdown: OpenDropdown = {
      select, menu, options: [], rows: [], active: -1, query: "", lastTyped: 0, observer,
    };
    open = dropdown;
    select.setAttribute("aria-controls", menu.id);
    select.setAttribute("aria-expanded", "true");
    renderOptions(dropdown);
    observer.observe(select, {
      childList: true, subtree: true, characterData: true, attributes: true,
      attributeFilter: ["label", "disabled", "selected", "multiple", "size", "data-native-picker"],
    });
    for (let ancestor = select.parentElement; ancestor; ancestor = ancestor.parentElement) {
      observer.observe(ancestor, {
        childList: true, attributes: true, attributeFilter: ["inert", "hidden", "open", "disabled"],
      });
      if (ancestor === root) break;
    }

    // Keep the combobox focused while an option is clicked.
    menu.addEventListener("mousedown", event => event.preventDefault());
    menu.addEventListener("click", event => {
      if (open !== dropdown || !(event.target instanceof Element)) return;
      const row = event.target.closest('[role="option"]');
      const index = dropdown.rows.findIndex(option => option === row);
      if (index >= 0 && !dropdown.options[index].disabled) commit(dropdown, index);
    });
    return dropdown;
  }

  root.addEventListener("mousedown", event => {
    if (event.button === 0 && !event.defaultPrevented && dropdownTarget(event.target)) event.preventDefault();
  }, { capture: true, signal });

  root.addEventListener("click", event => {
    const select = dropdownTarget(event.target);
    if (!select || event.defaultPrevented || event.button !== 0) return;
    event.preventDefault();
    if (open?.select === select) {
      close();
      return;
    }
    select.focus({ preventScroll: true });
    show(select);
  }, { capture: true, signal });

  root.addEventListener("keydown", event => {
    const select = dropdownTarget(event.target);
    if (!select || event.defaultPrevented || event.isComposing || event.ctrlKey || event.metaKey) return;
    const current = open?.select === select ? open : null;
    if (event.key === "Tab") {
      if (current) commit(current, current.active);
      return;
    }
    const printable = event.key.length === 1 && event.key !== " " && !event.altKey;
    const opens = [" ", "Enter", "ArrowDown", "ArrowUp", "Home", "End"].includes(event.key);
    const dismisses = event.key === "Escape" || event.altKey && event.key === "ArrowUp";
    if (dismisses ? !current : !printable && !opens) return;
    event.preventDefault();
    event.stopPropagation();
    if (dismisses) {
      close();
      return;
    }
    if (current && [" ", "Enter"].includes(event.key)) {
      commit(current, current.active);
      return;
    }
    const dropdown = current ?? show(select);
    const { options, active } = dropdown;
    const enabled = options.flatMap((option, index) => option.disabled ? [] : [index]);
    if (!enabled.length) return;
    if (event.key === "Home" || event.key === "End") {
      setActive(dropdown, event.key === "Home" ? enabled[0] : enabled[enabled.length - 1]);
    } else if (current && !event.altKey && ["ArrowDown", "ArrowUp"].includes(event.key)) {
      const offset = event.key === "ArrowDown" ? 1 : -1;
      const next = Math.max(0, Math.min(enabled.indexOf(active) + offset, enabled.length - 1));
      setActive(dropdown, enabled[next]);
    } else if (printable) {
      const now = Date.now();
      const key = event.key.toLocaleLowerCase();
      const previous = now - dropdown.lastTyped < typeaheadDelayMs ? dropdown.query : "";
      dropdown.lastTyped = now;
      dropdown.query = previous + key;
      const repeated = previous.length > 0 && Array.from(previous).every(letter => letter === key);
      const query = repeated ? key : dropdown.query;
      const start = enabled.indexOf(active);
      const searchFrom = start + (!previous || repeated ? 1 : 0);
      const ordered = [...enabled.slice(searchFrom), ...enabled.slice(0, searchFrom)];
      const match = ordered.find(index => options[index].label.trim().toLocaleLowerCase().startsWith(query));
      if (match !== undefined) setActive(dropdown, match);
    }
  }, { capture: true, signal });

  function closeOutside(event: Event) {
    if (open && event.target instanceof Node && event.target !== open.select && !open.menu.contains(event.target)) close();
  }

  document.addEventListener("pointerdown", closeOutside, { capture: true, signal });
  document.addEventListener("focusin", closeOutside, { capture: true, signal });
  window.addEventListener("resize", close, { signal });
  document.addEventListener("scroll", event => {
    if (open && event.target instanceof Node && !open.menu.contains(event.target)) close();
  }, { capture: true, signal });

  return () => {
    close();
    root.removeAttribute("data-burrata-selects");
    listeners.abort();
  };
}
