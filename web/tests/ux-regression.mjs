import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";

const app = await readFile(new URL("../src/app.tsx", import.meta.url), "utf8");
const api = await readFile(new URL("../src/api.ts", import.meta.url), "utf8");
const css = await readFile(new URL("../src/style.css", import.meta.url), "utf8");

test("secure pairing reads a fragment secret once, immediately removes it, and keeps bounded legacy query support", () => {
  assert.match(app, /new URLSearchParams\(location\.search\)/);
  assert.match(app, /new URLSearchParams\(location\.hash\.slice\(1\)\)/);
  assert.match(app, /const fragmentSecret = fragment\.get\("secret"\) \?\? ""/);
  assert.match(app, /LEGACY_QUERY_SECRET_ROLLOUT_END/);
  assert.match(app, /Date\.now\(\) <= LEGACY_QUERY_SECRET_ROLLOUT_END \? legacySecret : ""/);
  assert.match(app, /params\.delete\("secret"\);[\s\S]*history\.replaceState/);
  assert.match(app, /const handoff = useRef\(\{[\s\S]*approvalSecret/);
  assert.match(app, /approvalSecret: inMemoryApprovalSecret/);
  assert.doesNotMatch(app, /console\.(log|debug|info|warn|error)\(/);
});

test("secure pairing keeps only its current URL context and cannot approve from a terminal state", () => {
  assert.match(app, /const isPairing = Boolean\(code && approvalSecret\)/);
  assert.match(app, /isPairing && !showCabinet\) void lookup\(\)/);
  assert.match(app, /disabled=\{pairingState !== "authenticated"\}/);
  assert.match(app, /setPairingState\("approved"\)/);
  assert.match(app, /setPairingState\("terminal"\)/);
});

test("a restored browser cookie requires a fresh passkey before pairing approval", () => {
  assert.match(app, /current === "approved" \|\| current === "authenticated" \|\| current === "approving" \? current : "anonymous"/);
  assert.match(app, /Для подключения устройства требуется свежая проверка passkey/);
  assert.match(app, /Подтвердить passkey/);
  assert.match(app, /pairingState === "authenticated" \|\| pairingState === "approving"/);
});

test("registration remains distinct from browser authentication", () => {
  assert.match(app, /const \[registrationName, setRegistrationName\]/);
  assert.match(app, /const \[authenticatedAccountName, setAuthenticatedAccountName\]/);
  assert.match(app, /onInput=\{event => setRegistrationName\(event\.currentTarget\.value\)\}/);
  assert.match(app, /Создать Rock-аккаунт/);
  assert.doesNotMatch(app, /onInput=\{event => setAuthenticatedAccountName/);
});

test("cabinet has exclusive loading, anonymous, authenticated, expired, and unavailable states", () => {
  assert.match(app, /type AccountState = "loading" \| "anonymous" \| "authenticated" \| "expired" \| "unavailable"/);
  assert.match(app, /accountState === "loading"/);
  assert.match(app, /accountState === "unavailable"/);
  assert.match(app, /accountState === "expired"/);
  assert.match(app, /accountState === "anonymous"/);
  assert.match(app, /Сессия браузера завершена/);
  assert.match(app, /Сервис временно недоступен/);
});

test("account loading fetches a fresh browser session and exact current device projection", () => {
  assert.match(api, /browserSession\(\).*\/v1\/auth\/browser-session/);
  assert.match(api, /browserAccount\(\).*\/v1\/browser\/account/);
  assert.match(app, /const nextAccount = await api\.browserAccount\(\)/);
  assert.match(app, /setAccount\(undefined\); setCsrf\(""/);
  assert.match(app, /setAccountState\("expired"\)/);
});

test("device actions are confirmed, independently busy, and refresh account data", () => {
  assert.match(api, /renameDevice\(.*\/v1\/browser\/devices/);
  assert.match(api, /revokeDevice\(.*\/v1\/browser\/devices/);
  assert.match(app, /const \[deviceBusy, setDeviceBusy\]/);
  assert.match(app, /const \[logoutBusy, setLogoutBusy\]/);
  assert.match(app, /await api\.renameDevice[\s\S]*await refreshAccount\(\)/);
  assert.match(app, /await api\.revokeDevice[\s\S]*await refreshAccount\(\)/);
  assert.match(app, /Это не завершит вход в текущем браузере/);
});

test("cabinet explains browser safety and distinguishes empty and full device limits", () => {
  assert.match(app, /Выполнен вход в браузере/);
  assert.match(app, /Браузер подтверждает устройство, но не является RockMobile или RockCast/);
  assert.match(app, /Подключённых устройств пока нет/);
  assert.match(app, /Лимит устройств достигнут/);
  assert.match(app, /Passkey подтверждает вход в этот браузер/);
  assert.match(app, /не удаляет passkey из браузера или Google Password Manager/);
});

test("device presentation renders one product prefix and no secret or identifier DOM fields", () => {
  assert.match(app, /deviceName\(device\)/);
  assert.doesNotMatch(app, /\{preview\.request_id\}/);
  assert.doesNotMatch(app, /access_token|refresh_token|desktop_token|approval_secret/);
});

test("pairing success offers cabinet handoff with an in-memory one-time connected marker", () => {
  assert.match(app, /const \[justConnected, setJustConnected\]/);
  assert.match(app, /setShowCabinet\(true\)/);
  assert.match(app, /Только что подключено/);
  assert.match(app, /Вернуться в RockMobile/);
  assert.match(app, /Вернитесь в RockCast или закройте браузер/);
});

test("accessible controls have semantic status and visible focus styles across mobile layouts", () => {
  assert.match(app, /role="alert"/);
  assert.match(app, /role="status"/);
  assert.match(app, /aria-label=\{.*Проверочная фраза/);
  assert.match(css, /:focus-visible/);
  assert.match(css, /@media \(max-width: 480px\)/);
  assert.match(css, /button, \.button \{ width: 100%/);
});
