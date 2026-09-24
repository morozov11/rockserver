import { useEffect, useRef, useState } from "preact/hooks";
import { api, browserAuthenticationOptions, browserRegistrationOptions, serializeAuthentication, serializeRegistration, type ApiError, type BrowserAccount, type BrowserDevice, type PairingPreview, type YandexHomeSensor } from "./api";
import { AdminApp } from "./admin";
import "./style.css";

type PairingState = "loading" | "anonymous" | "authenticated" | "approving" | "approved" | "terminal" | "unavailable";
type AccountState = "loading" | "anonymous" | "authenticated" | "expired" | "unavailable";
type JustConnected = Pick<BrowserDevice, "device_display_name" | "device_type">;

const errorMessage = (error: unknown) => {
  const code = (error as ApiError)?.code;
  if (code === "pairing_not_found" || code === "pairing_not_approvable") return "Этот запрос на подключение истёк, уже завершён или устройство уже подключено.";
  if (code === "auth_unavailable" || code === "pairing_unavailable" || code === "server_unavailable") return "Сервер временно недоступен. Повторите попытку позже.";
  return "Не удалось выполнить действие. Повторите попытку позже.";
};
const passkeyErrorMessage = (error: unknown) => {
  if (error instanceof DOMException) {
    if (error.name === "AbortError") return "Вход отменён пользователем.";
    if (error.name === "NotAllowedError") return "Ключ не найден. Выберите passkey этого Rock-аккаунта и попробуйте ещё раз.";
  }
  if ((error as ApiError)?.code === "webauthn_rejected") return "Ключ не найден. Выберите passkey этого Rock-аккаунта и попробуйте ещё раз.";
  return (error as ApiError)?.code ? "Сервер временно недоступен. Повторите попытку позже." : "Не удалось выполнить вход с passkey.";
};
const registrationErrorMessage = (error: unknown) => error instanceof DOMException && (error.name === "AbortError" || error.name === "NotAllowedError") ? "Создание отменено. Имя аккаунта сохранено — попробуйте ещё раз, когда будете готовы." : errorMessage(error);
const deviceProductName = (deviceType: string) => deviceType.toLowerCase().includes("mobile") ? "RockMobile" : "RockCast";
const deviceName = (device: Pick<BrowserDevice, "device_type" | "device_display_name">) => {
  const product = deviceProductName(device.device_type);
  return device.device_display_name.startsWith(`${product} — `) ? device.device_display_name : `${product} — ${device.device_display_name}`;
};
const formatDate = (value: string) => new Intl.DateTimeFormat("ru-RU", { dateStyle: "medium", timeStyle: "short" }).format(new Date(value));
const isAuthenticationError = (error: unknown) => (error as ApiError)?.code === "authentication_required";
const LEGACY_QUERY_SECRET_ROLLOUT_END = Date.parse("2026-09-29T00:00:00Z");

/** Selects the same-origin administrator SPA without changing public browser routes. */
export function App() {
  return location.pathname === "/admin" ? <AdminApp /> : <PublicApp />;
}

/** Renders the account landing page or the secure, request-specific pairing screen. */
function PublicApp() {
  const params = new URLSearchParams(location.search);
  const parsedCode = params.get("code")?.trim().toUpperCase() ?? "";
  const fragment = new URLSearchParams(location.hash.slice(1));
  const fragmentSecret = fragment.get("secret") ?? "";
  const legacySecret = params.get("secret") ?? "";
  const yandexHomeParam = params.get("yandex_home") ?? "";
  // Keep the old query handoff only through the bounded rollout window.
  const parsedApprovalSecret = fragmentSecret || (Date.now() <= LEGACY_QUERY_SECRET_ROLLOUT_END ? legacySecret : "");
  if (fragmentSecret || legacySecret) {
    params.delete("secret");
    history.replaceState(null, "", `${location.pathname}${params.size ? `?${params}` : ""}`);
  }
  if (yandexHomeParam) {
    params.delete("yandex_home");
    history.replaceState(null, "", `${location.pathname}${params.size ? `?${params}` : ""}`);
  }
  const [yandexHomeStatus] = useState(yandexHomeParam);
  const handoff = useRef({
    code: parsedCode,
    approvalSecret: parsedApprovalSecret,
    pairingSearch: params.size ? `?${params}` : "",
  });
  const { code, approvalSecret: inMemoryApprovalSecret, pairingSearch } = handoff.current;
  const approvalSecret = inMemoryApprovalSecret;
  const isPairing = Boolean(code && approvalSecret);
  const [screen, setScreen] = useState<"main" | "register">(() => location.pathname === "/register" ? "register" : "main");
  const [preview, setPreview] = useState<PairingPreview>();
  const [message, setMessage] = useState("");
  const [accountMessage, setAccountMessage] = useState("");
  const [csrf, setCsrf] = useState("");
  const [registrationName, setRegistrationName] = useState("");
  const [authenticatedAccountName, setAuthenticatedAccountName] = useState("");
  const [pairingState, setPairingState] = useState<PairingState>("loading");
  const [accountState, setAccountState] = useState<AccountState>("loading");
  const [authBusy, setAuthBusy] = useState(false);
  const [deviceBusy, setDeviceBusy] = useState("");
  const [logoutBusy, setLogoutBusy] = useState(false);
  const [account, setAccount] = useState<BrowserAccount>();
  const [registrationComplete, setRegistrationComplete] = useState(false);
  const [showCabinet, setShowCabinet] = useState(false);
  const [justConnected, setJustConnected] = useState<JustConnected>();
  const registrationBusy = useRef(false);

  const lookup = async () => {
    try { setPreview(await api.pairing(code)); setPairingState(current => current === "loading" ? "anonymous" : current); }
    catch (error) { setPreview(undefined); setPairingState((error as ApiError)?.code === "server_unavailable" ? "unavailable" : "terminal"); setMessage(errorMessage(error)); }
  };
  const loadAccount = async (expiredOnUnauthorized = false) => {
    try {
      const session = await api.browserSession();
      const nextAccount = await api.browserAccount();
      setCsrf(session.csrf_token); setAuthenticatedAccountName(session.account_display_name); setAccount(nextAccount); setAccountState("authenticated");
    } catch (error) {
      setAccount(undefined); setCsrf(""); setAuthenticatedAccountName("");
      if (isAuthenticationError(error)) setAccountState(expiredOnUnauthorized ? "expired" : "anonymous");
      else { setAccountState("unavailable"); setAccountMessage(errorMessage(error)); }
    }
  };
  const restoreSession = async () => {
    if (!isPairing || showCabinet) { await loadAccount(); return; }
    try {
      const session = await api.browserSession();
      setCsrf(session.csrf_token); setAuthenticatedAccountName(session.account_display_name);
      // A restored cookie is not a fresh passkey assertion, so it cannot approve a pairing.
      setPairingState(current => current === "approved" || current === "authenticated" || current === "approving" ? current : "anonymous");
    } catch (error) {
      if (!isAuthenticationError(error)) { setPairingState("unavailable"); setMessage(errorMessage(error)); }
    }
  };
  useEffect(() => { void restoreSession(); if (isPairing && !showCabinet) void lookup(); }, [showCabinet]);

  const register = async () => {
    if (registrationBusy.current) return;
    if (!registrationName.trim()) { setMessage("Введите имя аккаунта или выберите «Rock account»."); return; }
    registrationBusy.current = true; setAuthBusy(true); setMessage("");
    try {
      if (!window.PublicKeyCredential) throw new Error("Браузер не поддерживает passkey.");
      const started = await api.registrationOptions({ account_display_name: registrationName.trim() });
      const credential = await navigator.credentials.create({ publicKey: browserRegistrationOptions(started) });
      if (!(credential instanceof PublicKeyCredential)) throw new Error("Passkey не создан.");
      const result = await api.registrationVerify({ challenge_id: started.challenge_id, ...serializeRegistration(credential) });
      setCsrf(result.csrf_token); setAuthenticatedAccountName(result.account_display_name);
      if (isPairing) { history.replaceState(null, "", `/${pairingSearch}`); setScreen("main"); setMessage("Rock-аккаунт создан, вход выполнен. Теперь подтвердите показанное устройство."); await lookup(); setPairingState("authenticated"); }
      else setRegistrationComplete(true);
    } catch (error) { setMessage(registrationErrorMessage(error)); } finally { registrationBusy.current = false; setAuthBusy(false); }
  };
  const authenticate = async () => {
    if (!window.PublicKeyCredential) { setMessage("Браузер не поддерживает passkey."); return; }
    setAuthBusy(true); setMessage("");
    try {
      const started = await api.authenticationOptions();
      const credential = await navigator.credentials.get({ publicKey: browserAuthenticationOptions(started) });
      if (!(credential instanceof PublicKeyCredential)) { setMessage("Ключ не найден. Выберите passkey этого Rock-аккаунта и попробуйте ещё раз."); return; }
      const result = await api.authenticationVerify({ challenge_id: started.challenge_id, ...serializeAuthentication(credential) });
      setCsrf(result.csrf_token);
      if (isPairing && !showCabinet) { await lookup(); const session = await api.browserSession(); setCsrf(session.csrf_token); setAuthenticatedAccountName(session.account_display_name); setPairingState("authenticated"); }
      else await loadAccount();
      setMessage(isPairing && !showCabinet ? "Вход выполнен. Проверьте устройство перед подключением." : "Вход выполнен.");
    } catch (error) { setMessage(passkeyErrorMessage(error)); } finally { setAuthBusy(false); }
  };
  const approve = async () => {
    if (!preview || !authenticatedAccountName || !csrf || pairingState !== "authenticated") { setMessage("Сначала войдите с passkey."); return; }
    setAuthBusy(true); setPairingState("approving"); setMessage("");
    try { await api.approvePairing(preview.request_id, approvalSecret, preview.verification_phrase, csrf); setPairingState("approved"); }
    catch (error) { setPairingState("terminal"); setMessage(errorMessage(error)); } finally { setAuthBusy(false); }
  };
  const refreshAccount = async () => { setAccountState("loading"); setAccountMessage(""); await loadAccount(true); };
  const rename = async (device: BrowserDevice) => {
    const name = window.prompt("Новое имя устройства", device.device_display_name)?.trim();
    if (!name || name === device.device_display_name) return;
    setDeviceBusy(device.device_id); setAccountMessage("");
    try { await api.renameDevice(device.device_id, name, csrf); await refreshAccount(); setAccountMessage("Имя устройства обновлено."); }
    catch (error) { if (isAuthenticationError(error)) { setAccount(undefined); setAccountState("expired"); } else setAccountMessage(errorMessage(error)); } finally { setDeviceBusy(""); }
  };
  const revoke = async (device: BrowserDevice) => {
    const name = deviceName(device);
    if (!window.confirm(`Отключить «${name}»? На нём будет завершён вход в RockCast или RockMobile. Это не завершит вход в текущем браузере.`)) return;
    setDeviceBusy(device.device_id); setAccountMessage("");
    try { await api.revokeDevice(device.device_id, csrf); await refreshAccount(); setAccountMessage("Устройство отключено."); }
    catch (error) { if (isAuthenticationError(error)) { setAccount(undefined); setAccountState("expired"); } else setAccountMessage(errorMessage(error)); } finally { setDeviceBusy(""); }
  };
  const logout = async () => {
    if (!window.confirm("Выйти из Rock-аккаунта в этом браузере? Устройства останутся подключёнными.")) return;
    setLogoutBusy(true); setAccountMessage("");
    try { await api.logoutBrowser(csrf); setCsrf(""); setAuthenticatedAccountName(""); setAccount(undefined); setAccountState("anonymous"); setAccountMessage("Вы вышли из аккаунта в этом браузере."); }
    catch (error) { if (isAuthenticationError(error)) { setAccount(undefined); setAccountState("expired"); } else setAccountMessage(errorMessage(error)); } finally { setLogoutBusy(false); }
  };
  const openRegistration = () => { history.pushState(null, "", `/register${pairingSearch}`); setMessage(""); setScreen("register"); };
  const returnFromRegistration = () => { history.replaceState(null, "", `/${pairingSearch}`); setMessage(""); setScreen("main"); };
  const openCabinet = () => {
    if (preview) setJustConnected({ device_display_name: preview.device_display_name, device_type: preview.device_type });
    history.replaceState(null, "", "/"); setShowCabinet(true); setAccountState("loading");
  };

  if (screen === "register") return <main><header><span>ROCK</span><h1>Создать Rock-аккаунт</h1></header>
    {registrationComplete ? <section><p className="eyebrow">Аккаунт создан</p><h2>Вход в браузере выполнен</h2><p>Rock-аккаунт «{authenticatedAccountName}» создан и защищён passkey.</p><a className="button" href="/">Открыть аккаунт и устройства</a></section>
      : <section><p>Создайте новый Rock-аккаунт с passkey. Для входа в существующий аккаунт используйте отдельный вход.</p><label htmlFor="account-name">Имя аккаунта <span className="example">Например, Алексей</span></label><input id="account-name" value={registrationName} maxLength={128} placeholder="Например, Алексей" disabled={authBusy} onInput={event => setRegistrationName(event.currentTarget.value)} />
        <button className="secondary" onClick={() => setRegistrationName("Rock account")} disabled={authBusy}>Использовать «Rock account»</button><button onClick={register} disabled={authBusy}>{authBusy ? "Создаём…" : "Создать аккаунт с passkey"}</button><button className="link-button" onClick={returnFromRegistration} disabled={authBusy}>У меня уже есть аккаунт</button></section>}
    {message && <p role="alert">{message}</p>}<footer>Passkey и данные сессии не сохраняются в браузере.</footer></main>;

  if (!isPairing || showCabinet) return <AccountCentre account={account} accountState={accountState} accountName={authenticatedAccountName} accountMessage={accountMessage || message} csrf={csrf} authBusy={authBusy} deviceBusy={deviceBusy} logoutBusy={logoutBusy} justConnected={justConnected} yandexHomeStatus={yandexHomeStatus} onAuthenticate={authenticate} onRegister={openRegistration} onRetry={refreshAccount} onRename={rename} onRevoke={revoke} onLogout={logout} />;
  const deviceType = deviceProductName(preview?.device_type ?? "");
  return <main><header><span>ROCK</span><h1>Подключение устройства</h1></header>
    {preview && pairingState !== "approved" && <section><p className="eyebrow">Проверьте, что это ваше устройство</p><h2>{deviceName(preview)}</h2><dl><div><dt>Проверочная фраза</dt><dd aria-label={`Проверочная фраза: ${preview.verification_phrase}`}>{preview.verification_phrase}</dd></div><div><dt>Короткий код</dt><dd aria-label={`Короткий код: ${preview.short_code}`}>{preview.short_code}</dd></div><div><dt>Действует до</dt><dd>{formatDate(preview.expires_at)}</dd></div></dl></section>}
    {message && <p role="alert">{message}</p>}
    {preview && pairingState === "approved" ? <section><p className="eyebrow">✓ Устройство подключено</p><h2>{deviceName(preview)} подключён к «{authenticatedAccountName}»</h2>{deviceType === "RockMobile" ? <a className="button" href="/return/rockmobile">Вернуться в RockMobile</a> : <p role="status">Вернитесь в RockCast или закройте браузер.</p>}<button className="secondary" onClick={openCabinet}>Открыть аккаунт и устройства</button></section>
      : preview && authenticatedAccountName && csrf && (pairingState === "authenticated" || pairingState === "approving") ? <section><h2>Подключить {deviceType} к аккаунту «{authenticatedAccountName}»?</h2><p>Будет подключено только показанное выше устройство.</p><button onClick={approve} disabled={pairingState !== "authenticated"}>{pairingState === "approving" ? "Подключаем…" : "Подключить"}</button><a className="button secondary" href="/">Отмена</a></section>
      : preview ? <section><h2>{authenticatedAccountName ? `Подтвердите вход в «${authenticatedAccountName}»` : "Чтобы продолжить"}</h2><p>{authenticatedAccountName ? "Для подключения устройства требуется свежая проверка passkey." : "Войдите в существующий Rock-аккаунт или создайте новый. После этого вы вернётесь к этому устройству."}</p><button onClick={authenticate} disabled={authBusy}>{authBusy ? "Проверяем…" : authenticatedAccountName ? "Подтвердить passkey" : "Войти с passkey"}</button>{authenticatedAccountName ? <><p>Если passkey этого аккаунта удалён, восстановить его без сохранённого ключа нельзя.</p><button className="secondary" onClick={openRegistration} disabled={authBusy}>Создать другой Rock-аккаунт</button></> : <button className="secondary" onClick={openRegistration} disabled={authBusy}>Создать Rock-аккаунт</button>}</section>
      : <section><h2>Ссылка подключения недействительна</h2><p>Откройте новую защищённую ссылку на устройстве, которое хотите подключить.</p></section>}
    <footer>Passkey и данные сессии не сохраняются в браузере.</footer></main>;
}

/** Renders the safe browser account and native-device cabinet. */
function AccountCentre({ account, accountState, accountName, accountMessage, csrf, authBusy, deviceBusy, logoutBusy, justConnected, yandexHomeStatus, onAuthenticate, onRegister, onRetry, onRename, onRevoke, onLogout }: { account?: BrowserAccount; accountState: AccountState; accountName: string; accountMessage: string; csrf: string; authBusy: boolean; deviceBusy: string; logoutBusy: boolean; justConnected?: JustConnected; yandexHomeStatus?: string; onAuthenticate: () => Promise<void>; onRegister: () => void; onRetry: () => Promise<void>; onRename: (device: BrowserDevice) => Promise<void>; onRevoke: (device: BrowserDevice) => Promise<void>; onLogout: () => Promise<void>; }) {
  if (accountState === "loading") return <main aria-busy="true"><header><span>ROCK</span><h1>Rock-аккаунт</h1></header><section role="status"><h2>Загружаем аккаунт…</h2><p>Проверяем вход в этом браузере.</p></section></main>;
  if (accountState === "unavailable") return <main><header><span>ROCK</span><h1>Rock-аккаунт</h1></header><section role="alert"><h2>Сервис временно недоступен</h2><p>{accountMessage || "Попробуйте обновить данные позже."}</p><button onClick={onRetry}>Повторить</button></section></main>;
  if (accountState === "expired") return <main><header><span>ROCK</span><h1>Rock-аккаунт</h1></header><section role="alert"><h2>Сессия браузера завершена</h2><p>Войдите с passkey ещё раз, чтобы увидеть устройства.</p><button onClick={onAuthenticate} disabled={authBusy}>{authBusy ? "Проверяем…" : "Войти с passkey"}</button></section></main>;
  if (accountState === "anonymous") return <main><header><span>ROCK</span><h1>Rock-аккаунт</h1></header><section><h2>Вы не вошли</h2><p>Вход открывает существующий Rock-аккаунт. Создание аккаунта создаёт новый аккаунт с passkey.</p><button onClick={onAuthenticate} disabled={authBusy}>{authBusy ? "Проверяем…" : "Войти с passkey"}</button><button className="secondary" onClick={onRegister} disabled={authBusy}>Создать Rock-аккаунт</button>{accountMessage && <p role="status">{accountMessage}</p>}</section><footer>Passkey и данные сессии не сохраняются в браузере.</footer></main>;
  if (!account) return null;
  const limitReached = account.devices.length >= account.device_limit;
  return <main><header><span>ROCK</span><h1>Rock-аккаунт «{accountName}»</h1><p className="badge" role="status">✓ Выполнен вход в браузере</p></header>
    <section>
      <h2>Подключённые устройства ({account.devices.length})</h2>
      {limitReached && <p role="status">⚠ Лимит устройств достигнут ({account.device_limit}). Сначала отключите старое устройство.</p>}
      {account.devices.length === 0 ? (
        <p>Подключённых устройств пока нет. Откройте RockMobile или RockCast и начните подключение там.</p>
      ) : (
        <ul className="devices" aria-label="Список подключённых устройств">
          {account.devices.map(device => {
            const fresh = justConnected?.device_display_name === device.device_display_name && justConnected.device_type === device.device_type;
            const busy = deviceBusy === device.device_id;
            return (
              <li key={device.device_id}>
                {fresh && <p className="fresh" role="status">✓ Только что подключено</p>}
                <div>
                  <strong>{deviceName(device)}</strong>
                  <p>{device.session_status === "active" ? "● Сессия активна" : "○ Нет активной сессии"} · Подключено {formatDate(device.connected_at)}{device.last_seen_at && ` · Активность ${formatDate(device.last_seen_at)}`}</p>
                </div>
                <div className="device-actions">
                  <button className="secondary" onClick={() => onRename(device)} disabled={busy}>{busy ? "Обновляем…" : "Переименовать"}</button>
                  <button className="danger" onClick={() => onRevoke(device)} disabled={busy}>Отключить</button>
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </section>
    <YandexHome account={account} csrf={csrf} onChanged={onRetry} initialStatus={yandexHomeStatus} />
    <section><h2>Как подключить новое устройство</h2><p>Откройте RockMobile или RockCast на устройстве и начните подключение из приложения. Браузер подтверждает устройство, но не является RockMobile или RockCast и не считается текущим native-устройством.</p></section>
    <section><h2>Безопасность доступа</h2><p>Passkey подтверждает вход в этот браузер. «Отключить» завершает native-сессии выбранного устройства, но не завершает вход в текущем браузере; для него используйте действие ниже.</p><p>Сервер не удаляет passkey из браузера или Google Password Manager. Старый ключ удаляйте вручную только после успешного входа новым ключом. Одинаковое имя «RockServer user» само по себе не доказывает, что запись старая.</p></section>
    <section><h2>Вход в браузере</h2><button className="secondary" onClick={onLogout} disabled={logoutBusy}>{logoutBusy ? "Выходим…" : "Выйти из браузера"}</button>{accountMessage && <p role="status">{accountMessage}</p>}</section><footer>Passkey и данные сессии не сохраняются в браузере.</footer></main>;
}

/** Lets an authenticated account link Yandex Smart Home and read its temperature properties. */
function YandexHome({ account, csrf, onChanged, initialStatus }: { account: BrowserAccount; csrf: string; onChanged: () => Promise<void>; initialStatus?: string }) {
  const [sensors, setSensors] = useState<YandexHomeSensor[]>([]);
  const [loading, setLoading] = useState(account.yandex_home_connected);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState(() => {
    if (initialStatus === "failed") return "Не удалось подключить Яндекс Дом. Повторите попытку.";
    if (initialStatus === "connected") return "Яндекс Дом успешно подключён.";
    return "";
  });
  const load = async () => {
    setLoading(true); setMessage("");
    try { setSensors((await api.yandexHomeSensors()).sensors); }
    catch (error) { setSensors([]); setMessage((error as ApiError)?.code === "yandex_home_reconnect_required" ? "Доступ к Яндекс Дому истёк. Подключите его снова." : "Не удалось получить данные Яндекс Дома."); }
    finally { setLoading(false); }
  };
  useEffect(() => { if (account.yandex_home_connected) void load(); else { setSensors([]); setLoading(false); } }, [account.yandex_home_connected]);
  const connect = async () => {
    setBusy(true); setMessage("");
    try { location.assign((await api.yandexHomeAuthorization(csrf)).authorization_url); }
    catch { setMessage("Не удалось открыть подключение Яндекс Дома. Повторите позже."); setBusy(false); }
  };
  const disconnect = async () => {
    if (!window.confirm("Отключить Яндекс Дом? RockServer перестанет видеть показания датчиков.")) return;
    setBusy(true); setMessage("");
    try { await api.disconnectYandexHome(csrf); await onChanged(); }
    catch { setMessage("Не удалось отключить Яндекс Дом. Повторите позже."); }
    finally { setBusy(false); }
  };
  return <section><p className="eyebrow">Умный дом</p><h2>Яндекс Алиса</h2>{account.yandex_home_connected ? <><p>Яндекс Дом подключён. RockServer запрашивает только доступные показания температуры.</p>{loading ? <p role="status">Обновляем показания…</p> : sensors.length ? <ul className="sensors" aria-label="Температурные датчики Яндекс Дома">{sensors.map(sensor => <li key={`${sensor.room_name ?? ""}-${sensor.device_name}`}><strong>{sensor.device_name}</strong><span>{sensor.room_name && `${sensor.room_name} · `}{sensor.temperature} {sensor.unit}</span>{sensor.updated_at && <small>Обновлено {formatDate(sensor.updated_at)}</small>}</li>)}</ul> : !message && <p role="status">Датчиков температуры с доступными показаниями не найдено.</p>}<button className="secondary" onClick={() => void load()} disabled={loading || busy}>{loading ? "Обновляем…" : "Обновить показания"}</button><button className="danger" onClick={() => void disconnect()} disabled={busy}>{busy ? "Отключаем…" : "Отключить Яндекс Дом"}</button></> : <><p>Подключите аккаунт Яндекса, чтобы видеть температуру с датчиков, доступных в приложении «Дом с Алисой».</p><button onClick={() => void connect()} disabled={busy || !csrf}>{busy ? "Открываем Яндекс…" : "Подключить Яндекс Дом"}</button></>}{message && <p role="alert">{message}</p>}</section>;
}
