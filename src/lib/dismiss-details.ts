/** Give compact details menus the same outside-click and Escape behavior. */
export function dismissDetails(node: HTMLDetailsElement) {
  const outside = (event: PointerEvent) => {
    if (node.open && event.target instanceof Node && !node.contains(event.target)) node.open = false;
  };
  const escape = (event: KeyboardEvent) => {
    if (!node.open || event.key !== "Escape" || event.defaultPrevented) return;
    event.preventDefault();
    event.stopPropagation();
    node.open = false;
    node.querySelector<HTMLElement>("summary")?.focus();
  };
  document.addEventListener("pointerdown", outside);
  document.addEventListener("keydown", escape);
  return { destroy() {
    document.removeEventListener("pointerdown", outside);
    document.removeEventListener("keydown", escape);
  } };
}
