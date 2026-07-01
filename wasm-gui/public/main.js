// wasm-bindgen `--target web` emits an ES module + a default `init()` that fetches
// and instantiates the .wasm. Everything below runs only after init() resolves.
import init, { Node } from "./wasm/wasm_gui.js";

const $status = document.querySelector("#status");
const $id = document.querySelector("#endpoint-id");
const $ticket = document.querySelector("#ticket");
const $payload = document.querySelector("#payload");
const $connect = document.querySelector("#connect");
const $result = document.querySelector("#result");
const $reset = document.querySelector("#reset-identity");

await init();

// Persist the endpoint's secret key so the identity (endpoint ID) is stable
// across reloads. NOTE: this is a private key sitting in localStorage — fine for
// a demo, not for anything sensitive. Clear it with `localStorage.clear()` (or the
// reset below) to roll a fresh identity.
const SECRET_STORAGE_KEY = "iroh-gui:secret";

let node;
try {
  $status.textContent = "spawning iroh endpoint…";
  // null when absent → wasm sees `None` → generates a fresh key.
  node = await Node.spawn(localStorage.getItem(SECRET_STORAGE_KEY));
  // Persist (idempotent if it already existed; stores the new one otherwise).
  localStorage.setItem(SECRET_STORAGE_KEY, node.secret_hex());

  const id = node.endpoint_id();
  $id.textContent = id;
  // Click the ID to select it for copying.
  $id.addEventListener("click", () => getSelection().selectAllChildren($id));

  $connect.disabled = false;
  $status.textContent = "endpoint ready ✓";
} catch (err) {
  $id.textContent = "—";
  $status.textContent = `failed to spawn endpoint: ${err}`;
  console.error(err);
}

// Echo round-trip: send the message to the ticket's endpoint and show what comes
// back. Same `echo/0` protocol the CLI client and ESP32 servers speak.
$connect.addEventListener("click", async () => {
  const ticket = $ticket.value.trim();
  const payload = $payload.value;
  if (!ticket) {
    $result.textContent = "enter an endpoint ticket first";
    return;
  }

  $connect.disabled = true;
  $result.textContent = "connecting…";
  try {
    const echoed = await node.connect(ticket, payload);
    const ok = echoed === payload;
    $result.textContent = ok
      ? `✓ echo OK — received: ${JSON.stringify(echoed)}`
      : `received (differs from sent): ${JSON.stringify(echoed)}`;
    $result.classList.toggle("error", !ok);
  } catch (err) {
    $result.textContent = `failed: ${err}`;
    $result.classList.add("error");
    console.error(err);
  } finally {
    $connect.disabled = false;
  }
});

// Roll a fresh identity: drop the stored secret and reload so spawn generates a
// new key.
$reset.addEventListener("click", () => {
  localStorage.removeItem(SECRET_STORAGE_KEY);
  location.reload();
});
