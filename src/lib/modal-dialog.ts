export function modalDialog(node: HTMLDialogElement, onCancelRequest: () => void) {
  const restoreFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const onCancel = (event: Event) => {
    event.preventDefault();
    onCancelRequest();
  };
  node.addEventListener("cancel", onCancel);
  if (typeof node.showModal === "function") node.showModal();
  else node.setAttribute("open", "");
  return {
    destroy() {
      node.removeEventListener("cancel", onCancel);
      if (node.open && typeof node.close === "function") node.close();
      if (restoreFocus?.isConnected) restoreFocus.focus();
    },
  };
}
