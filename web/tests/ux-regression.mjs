import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";

const app = await readFile(new URL("../src/app.tsx", import.meta.url), "utf8");
const api = await readFile(new URL("../src/api.ts", import.meta.url), "utf8");

test("secure pairing keeps its code and secret context in the current URL", () => {
  assert.match(app, /new URLSearchParams\(location\.search\)/);
  assert.match(app, /const isPairing = Boolean\(code && approvalSecret\)/);
  assert.match(app, /if \(isPairing\) void lookup\(\)/);
  assert.match(app, /includes\("mobile"\)/);
});

test("pairing offers distinct sign-in and create-account choices without a technical code field", () => {
  assert.match(app, /Войти с passkey/);
  assert.match(app, /Создать Rock-аккаунт/);
  assert.match(app, /Создание аккаунта создаёт новый аккаунт с passkey/);
  assert.doesNotMatch(app, /<label>Код/);
  assert.doesNotMatch(app, /user_id/);
});

test("anonymous landing distinguishes existing-account sign-in from registration", () => {
  assert.match(app, /Вход открывает существующий Rock-аккаунт\. Создание аккаунта создаёт новый аккаунт с passkey\./);
  assert.match(app, /<button onClick=\{authenticate\}[^>]*>\{busy \? "Проверяем…" : "Войти с passkey"\}<\/button><button className="secondary" onClick=\{openRegistration\}/);
  assert.match(app, /history\.pushState\(null, "", `\/register\$\{location\.search\}`\)/);
  assert.doesNotMatch(app, /Код подключения/);
});

test("registration is a separate labelled form with an explicit default-name choice", () => {
  assert.match(app, /location\.pathname === "\/register"/);
  assert.match(app, /<h1>Создать Rock-аккаунт<\/h1>/);
  assert.match(app, /htmlFor="account-name">Имя аккаунта/);
  assert.match(app, /Например, Алексей/);
  assert.match(app, /Использовать «Rock account»/);
  assert.match(app, /У меня уже есть аккаунт/);
  assert.match(app, /if \(!registrationName\.trim\(\)\) \{ setMessage\("Введите имя аккаунта или выберите «Rock account»\."\); return; \}/);
  assert.match(app, /api\.registrationOptions\(\{ account_display_name: registrationName\.trim\(\) \}\)/);
});

test("registration starts only on click, blocks double submit, and preserves cancellation recovery", () => {
  assert.match(app, /const registrationBusy = useRef\(false\)/);
  assert.match(app, /if \(registrationBusy\.current\) return;/);
  assert.match(app, /registrationBusy\.current = true;/);
  assert.match(app, /registrationBusy\.current = false; setBusy\(false\)/);
  assert.match(app, /busy \? "Создаём…" : "Создать аккаунт с passkey"/);
  assert.match(app, /Создание отменено\. Имя аккаунта сохранено/);
  assert.match(app, /history\.replaceState\(null, "", `\/\$\{location\.search\}`\);[\s\S]*setScreen\("main"\);[\s\S]*await lookup\(\)/);
  assert.match(app, /registrationComplete \? <section><p className="eyebrow">Аккаунт создан<\/p><h2>Вход в браузере выполнен<\/h2>/);
});

test("an existing browser session receives a tab-local CSRF proof before approval", () => {
  assert.match(api, /browserSession\(\).*\/v1\/auth\/browser-session/);
  assert.match(app, /Подключить \{deviceType\} к аккаунту/);
  assert.match(app, /approvalSecret, preview\.verification_phrase, csrf/);
  assert.match(app, /!authenticatedAccountName \|\| !csrf \|\| pairingState !== "authenticated"/);
});

test("typing the first registration letter stays on the registration form", () => {
  assert.match(app, /const \[registrationName, setRegistrationName\]/);
  assert.match(app, /const \[authenticatedAccountName, setAuthenticatedAccountName\]/);
  assert.match(app, /onInput=\{event => setRegistrationName\(event\.currentTarget\.value\)\}/);
  assert.doesNotMatch(app, /onInput=\{event => setAuthenticatedAccountName/);
});

test("a 204 approval enters an exclusive success state with clear CTAs", () => {
  assert.match(app, /await api\.approvePairing[\s\S]*setPairingState\("approved"\)/);
  assert.match(app, /pairingState === "approved"/);
  assert.match(app, /Вернуться в RockMobile/);
  assert.match(app, /Вернитесь в RockCast/);
  assert.match(app, /Открыть аккаунт и устройства/);
  assert.match(app, /pairingState !== "approved"/);
});

test("an approved or consumed request reload cannot expose enabled approval", () => {
  assert.match(app, /setPreview\(undefined\);[\s\S]*setPairingState\(\(error as ApiError\)\?\.code === "server_unavailable" \? "unavailable" : "terminal"\)/);
  assert.match(app, /disabled=\{pairingState !== "authenticated"\}/);
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
  assert.match(app, /не удаляет passkey из браузера или Google Password Manager/);
  assert.match(app, /Одинаковое имя «RockServer user» само по себе не доказывает/);
  assert.doesNotMatch(app, /credential_id|refresh_token|access_token/);
});
