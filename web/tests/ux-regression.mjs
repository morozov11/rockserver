import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";

const app = await readFile(new URL("../src/app.tsx", import.meta.url), "utf8");
const api = await readFile(new URL("../src/api.ts", import.meta.url), "utf8");

test("secure pairing keeps its code and secret context in the current URL", () => {
  assert.match(app, /new URLSearchParams\(location\.search\)/);
  assert.match(app, /const isPairing = Boolean\(code && approvalSecret\)/);
  assert.match(app, /if \(isPairing\) await lookup\(\)/);
  assert.match(app, /includes\("mobile"\)/);
});

test("pairing offers distinct sign-in and create-account choices without a technical code field", () => {
  assert.match(app, /Войти с passkey/);
  assert.match(app, /Создать Rock-аккаунт/);
  assert.match(app, /создаст новый отдельный аккаунт/);
  assert.doesNotMatch(app, /<label>Код/);
  assert.doesNotMatch(app, /user_id/);
});

test("an existing browser session receives a tab-local CSRF proof before approval", () => {
  assert.match(api, /browserSession\(\).*\/v1\/auth\/browser-session/);
  assert.match(app, /Подключить \{deviceType\} к аккаунту/);
  assert.match(app, /approvalSecret, preview\.verification_phrase, csrf/);
});

test("unavailable API responses stay user-facing and never expose HTTP details", () => {
  assert.match(api, /code: "server_unavailable"/);
  assert.match(app, /Сервер временно недоступен/);
  assert.doesNotMatch(app, /HTTP-/);
});

test("authenticated landing is an account centre with safe device actions", () => {
  assert.match(api, /browserAccount\(\).*\/v1\/browser\/account/);
  assert.match(api, /renameDevice\(.*\/v1\/browser\/devices/);
  assert.match(api, /logoutBrowser\(.*\/v1\/auth\/browser-logout/);
  assert.match(app, /Устройства \(\{account\.devices\.length\} из \{account\.device_limit\}\)/);
  assert.match(app, /Выйти из браузера/);
  assert.match(app, /Отключить/);
  assert.match(app, /Это не завершит вход в текущем браузере/);
  assert.doesNotMatch(app, /credential_id|refresh_token|access_token/);
});
