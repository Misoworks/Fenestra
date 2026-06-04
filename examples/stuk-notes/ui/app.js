const defaultNotes = [
  {
    id: "product-notes",
    title: "Product notes",
    body: "Keep the app monochrome, calm, and functional.\nMake editing fast enough to feel native.",
    saved: true,
  },
  {
    id: "runtime-checklist",
    title: "Runtime checklist",
    body: "Install the embedded CEF runtime locally.\nShow a native installing window while it downloads.",
    saved: true,
  },
  {
    id: "design-pass",
    title: "Design pass",
    body: "Use square sidebars and restrained controls.\nKeep the editor focused on the note.",
    saved: true,
  },
];

const storeKey = "stuk.notes.example.v1";

let notes = loadNotes();
let selected = Math.min(loadSelected(), notes.length - 1);

const list = document.querySelector("#note-list");
const count = document.querySelector("#note-count");
const title = document.querySelector("#note-title");
const body = document.querySelector("#note-body");
const newButton = document.querySelector("#new-note");
const saveButton = document.querySelector("#save-note");
const deleteButton = document.querySelector("#delete-note");

function loadNotes() {
  try {
    const value = JSON.parse(localStorage.getItem(storeKey) || "null");
    if (Array.isArray(value?.notes) && value.notes.length > 0) {
      return value.notes.map((note) => ({
        id: String(note.id || crypto.randomUUID()),
        title: String(note.title || "Untitled"),
        body: String(note.body || ""),
        saved: true,
      }));
    }
  } catch {
  }
  return defaultNotes.map((note) => ({ ...note }));
}

function loadSelected() {
  try {
    const value = JSON.parse(localStorage.getItem(storeKey) || "null");
    return Number.isInteger(value?.selected) ? value.selected : 0;
  } catch {
    return 0;
  }
}

function persist() {
  localStorage.setItem(
    storeKey,
    JSON.stringify({
      selected,
      notes: notes.map(({ id, title, body }) => ({ id, title, body })),
    }),
  );
}

function currentNote() {
  return notes[selected];
}

async function invokeNative(name, params) {
  const bridge = window.fenestra?.bridge || window.stuk?.bridge;
  if (!bridge?.commands?.includes(name)) {
    return null;
  }
  try {
    return await bridge.invoke(name, params);
  } catch (error) {
    console.warn(error);
    return null;
  }
}

function renderList() {
  list.replaceChildren(
    ...notes.map((note, index) => {
      const button = document.createElement("button");
      button.textContent = note.saved ? note.title || "Untitled" : `${note.title || "Untitled"} *`;
      button.className = index === selected ? "active" : "";
      button.addEventListener("click", () => {
        persistCurrentFields();
        selected = index;
        persist();
        render();
      });
      return button;
    }),
  );
  count.textContent = `${notes.length} ${notes.length === 1 ? "note" : "notes"}`;
}

function renderEditor() {
  const note = currentNote();
  title.value = note.title;
  body.value = note.body;
  document.title = `${note.saved ? "" : "*"}Notes`;
}

function render() {
  renderList();
  renderEditor();
}

function persistCurrentFields() {
  const note = currentNote();
  note.title = title.value.trimStart();
  note.body = body.value;
}

title.addEventListener("input", () => {
  currentNote().title = title.value;
  currentNote().saved = false;
  renderList();
});

body.addEventListener("input", () => {
  currentNote().body = body.value;
  currentNote().saved = false;
});

newButton.addEventListener("click", async () => {
  persistCurrentFields();
  const created = await invokeNative("notes.create", { title: "Untitled", body: "" });
  notes.push({
    id: created?.id || crypto.randomUUID(),
    title: "Untitled",
    body: "",
    saved: false,
  });
  selected = notes.length - 1;
  persist();
  render();
  title.focus();
  title.select();
});

saveButton.addEventListener("click", () => {
  persistCurrentFields();
  currentNote().saved = true;
  persist();
  renderList();
});

deleteButton.addEventListener("click", () => {
  if (notes.length === 1) {
    notes[0] = { id: crypto.randomUUID(), title: "Untitled", body: "", saved: false };
    selected = 0;
  } else {
    notes.splice(selected, 1);
    selected = Math.min(selected, notes.length - 1);
  }
  persist();
  render();
});

document.addEventListener("keydown", (event) => {
  if (!(event.ctrlKey || event.metaKey)) {
    return;
  }
  if (event.key.toLowerCase() === "s") {
    event.preventDefault();
    saveButton.click();
  }
  if (event.key.toLowerCase() === "n") {
    event.preventDefault();
    newButton.click();
  }
});

window.addEventListener("beforeunload", () => {
  persistCurrentFields();
  persist();
});

render();
