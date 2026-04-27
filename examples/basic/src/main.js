import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

const $ = (id) => document.getElementById(id);

// --- greet ---------------------------------------------------------------
$('greet-form').addEventListener('submit', async (e) => {
  e.preventDefault();
  const name = $('greet-name').value;
  try {
    const reply = await invoke('greet', { name });
    $('greet-out').textContent = reply;
  } catch (err) {
    $('greet-out').textContent = `error: ${JSON.stringify(err)}`;
  }
});

// --- slow_double (Result<T,E>) ------------------------------------------
$('double-form').addEventListener('submit', async (e) => {
  e.preventDefault();
  const n = Number($('double-n').value);
  $('double-out').textContent = '…';
  try {
    const reply = await invoke('slow_double', { n });
    $('double-out').textContent = `ok: ${reply}`;
  } catch (err) {
    $('double-out').textContent = `err: ${err}`;
  }
});

// --- todos --------------------------------------------------------------
async function refreshTodos() {
  const todos = await invoke('list_todos');
  const ul = $('todo-list');
  ul.innerHTML = '';
  for (const t of todos) {
    const li = document.createElement('li');
    li.dataset.id = t.id;
    li.textContent = `#${t.id} — ${t.text}`;
    ul.appendChild(li);
  }
}

$('todo-form').addEventListener('submit', async (e) => {
  e.preventDefault();
  const text = $('todo-text').value.trim();
  if (!text) return;
  await invoke('add_todo', { args: { text } });
  $('todo-text').value = '';
});

$('todo-clear').addEventListener('click', async () => {
  await invoke('clear_todos');
});

await listen('todo-added', () => refreshTodos());
await listen('todos-cleared', () => refreshTodos());
await refreshTodos();

// --- tick stream --------------------------------------------------------
await listen('tick', (e) => {
  $('tick').textContent = String(e.payload);
});
