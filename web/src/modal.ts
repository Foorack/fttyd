// Minimal modal dialogs. Native <dialog> keeps focus handling, Escape and the backdrop
// behaviour of the platform instead of reimplementing them.

interface PromptOptions {
  title: string;
  label: string;
  value?: string;
  confirmLabel?: string;
}

interface ConfirmOptions {
  title: string;
  message: string;
  confirmLabel?: string;
  danger?: boolean;
}

function buildDialog(title: string): { dialog: HTMLDialogElement; body: HTMLElement; form: HTMLFormElement } {
  const dialog = document.createElement("dialog");
  dialog.className = "modal";

  const form = document.createElement("form");
  form.method = "dialog";

  const heading = document.createElement("h2");
  heading.textContent = title;
  form.appendChild(heading);

  const body = document.createElement("div");
  body.className = "modal-body";
  form.appendChild(body);

  dialog.appendChild(form);
  document.body.appendChild(dialog);
  return { dialog, body, form };
}

function addButtons(
  form: HTMLFormElement,
  confirmLabel: string,
  danger: boolean,
  confirmValue: string,
): HTMLButtonElement {
  const actions = document.createElement("div");
  actions.className = "modal-actions";

  const cancel = document.createElement("button");
  cancel.type = "button";
  cancel.className = "btn";
  cancel.textContent = "Cancel";
  cancel.addEventListener("click", () => {
    form.closest("dialog")?.close("cancel");
  });

  const confirm = document.createElement("button");
  confirm.type = "submit";
  confirm.className = danger ? "btn danger" : "btn primary";
  confirm.textContent = confirmLabel;
  confirm.value = confirmValue;
  confirm.dataset.role = "confirm";

  actions.append(cancel, confirm);
  form.appendChild(actions);
  return confirm;
}

export async function askText(options: PromptOptions): Promise<string | null> {
  const { dialog, body, form } = buildDialog(options.title);

  const label = document.createElement("label");
  label.className = "modal-label";
  label.textContent = options.label;

  const input = document.createElement("input");
  input.className = "modal-input";
  input.value = options.value ?? "";
  input.autocomplete = "off";
  input.spellcheck = false;
  label.appendChild(input);
  body.appendChild(label);

  addButtons(form, options.confirmLabel ?? "OK", false, "ok");
  dialog.returnValue = "cancel";
  dialog.showModal();
  input.focus();
  input.select();

  const result = await new Promise<string>((resolve) => {
    dialog.addEventListener("close", () => resolve(dialog.returnValue), { once: true });
  });
  const value = input.value.trim();
  dialog.remove();
  if (result !== "ok" || value === "") return null;
  return value;
}

export async function askConfirm(options: ConfirmOptions): Promise<boolean> {
  const { dialog, body, form } = buildDialog(options.title);
  const message = document.createElement("p");
  message.className = "modal-message";
  message.textContent = options.message;
  body.appendChild(message);

  addButtons(form, options.confirmLabel ?? "OK", options.danger ?? false, "ok");
  dialog.returnValue = "cancel";
  dialog.showModal();

  const result = await new Promise<string>((resolve) => {
    dialog.addEventListener("close", () => resolve(dialog.returnValue), { once: true });
  });
  dialog.remove();
  return result === "ok";
}
