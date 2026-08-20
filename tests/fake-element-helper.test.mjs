import assert from "node:assert/strict";
import test from "node:test";

import { FakeElementCore } from "./helpers/fake-element.mjs";

test("shared fake element core maintains attributes, events, and tree relationships", () => {
  const root = new FakeElementCore("main", { connected: true });
  const button = new FakeElementCore("button");
  button.setAttribute("role", "button");
  let clicks = 0;
  button.addEventListener("click", () => {
    clicks += 1;
  });

  root.appendChild(button);
  button.dispatchEvent({ type: "click" });

  assert.equal(clicks, 1);
  assert.equal(button.closest("main"), root);
  assert.equal(root.querySelector('button[role="button"]'), button);
  assert.equal(button.isConnected, true);

  button.remove();
  assert.equal(root.children.length, 0);
  assert.equal(button.isConnected, false);
});
