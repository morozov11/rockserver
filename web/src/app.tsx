import { useEffect, useState } from "preact/hooks";
import { api, browserAuthenticationOptions, browserRegistrationOptions, serializeAuthentication, serializeRegistration, type ApiError, type BrowserAccount, type PairingPreview } from "./api";
import "./style.css";

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

const deviceProductName = (deviceType: string) => deviceType.toLowerCase().includes("mobile") ? "RockMobile" : "RockCast";
const formatDate = (value: string) => new Intl.DateTimeFormat("ru-RU", { dateStyle: "medium", timeStyle: "short" }).format(new Date(value));
type PairingState = "loading" | "anonymous" | "authenticated" | "approving" | "approved" | "terminal" | "unavailable";

/** Renders the account landing page or the secure, request-specific pairing screen. */
export function App() {
  const params = new URLSearchParams(location.search);
  const code = params.get("code")?.trim().toUpperCase() ?? "";
  const approvalSecret = params.get("secret") ?? "";
  const isPairing = Boolean(code && approvalSecret);
  const [preview, setPreview] = useState<PairingPreview>();
  const [message, setMessage] = useState("");
  const [csrf, setCsrf] = useState("");
  const [registrationName, setRegistrationName] = useState("");
  const [authenticatedAccountName, setAuthenticatedAccountName] = useState("");
  const [pairingState, setPairingState] = useState<PairingState>("loading");
  const [busy, setBusy] = useState(false);
  const [account, setAccount] = useState<BrowserAccount>();

  const lookup = async () => {
    try {
      setPreview(await api.pairing(code));
      setPairingState(current => current === "loading" ? "anonymous" : current);
    } catch (error) {
      setPreview(undefined);
      setPairingState((error as ApiError)?.code === "server_unavailable" ? "unavailable" : "terminal");
      setMessage(errorMessage(error));
    }
  };
  const restoreSession = async () => {
    try {
      const session = await api.browserSession();
      setCsrf(session.csrf_token); setAuthenticatedAccountName(session.account_display_name);
      if (isPairing) setPairingState("authenticated");
      if (!isPairing) setAccount(await api.browserAccount());
    } catch (error) {
      if ((error as ApiError)?.code !== "authentication_required") setMessage(errorMessage(error));
    }
  };
  useEffect(() => { void restoreSession(); if (isPairing) void lookup(); }, []);

  const register = async () => {
    setBusy(true); setMessage("");
    try {
      if (!window.PublicKeyCredential) throw new Error("Браузер не поддерживает passkey.");
      const started = await api.registrationOptions(registrationName.trim() ? { account_display_name: registrationName.trim() } : undefined);
      const credential = await navigator.credentials.create({ publicKey: browserRegistrationOptions(started) });
      if (!(credential instanceof PublicKeyCredential)) throw new Error("Passkey не создан.");
      const result = await api.registrationVerify({ challenge_id: started.challenge_id, ...serializeRegistration(credential) });
      setCsrf(result.csrf_token); setAuthenticatedAccountName(result.account_display_name);
      setMessage("Rock-аккаунт создан. Теперь можно подтвердить устройство.");
      if (isPairing) await lookup();
    } catch (error) { setMessage(errorMessage(error)); } finally { setBusy(false); }
  };
  const authenticate = async () => {
    if (!window.PublicKeyCredential) { setMessage("Браузер не поддерживает passkey."); return; }
    setBusy(true); setMessage("");
    try {
      const started = await api.authenticationOptions();
      const credential = await navigator.credentials.get({ publicKey: browserAuthenticationOptions(started) });
      if (!(credential instanceof PublicKeyCredential)) { setMessage("Ключ не найден. Выберите passkey этого Rock-аккаунта и попробуйте ещё раз."); return; }
      const result = await api.authenticationVerify({ challenge_id: started.challenge_id, ...serializeAuthentication(credential) });
      setCsrf(result.csrf_token);
      if (isPairing) {
        await lookup();
        const session = await api.browserSession();
        setCsrf(session.csrf_token); setAuthenticatedAccountName(session.account_display_name); setPairingState("authenticated");
      }
      setMessage(isPairing ? "Вход выполнен. Проверьте устройство перед подключением." : "Вход выполнен.");
    } catch (error) { setMessage(passkeyErrorMessage(error)); } finally { setBusy(false); }
  };
  const approve = async () => {
    if (!preview || !authenticatedAccountName || !csrf || pairingState !== "authenticated") { setMessage("Сначала войдите с passkey."); return; }
    setBusy(true); setPairingState("approving"); setMessage("");
    try { await api.approvePairing(preview.request_id, approvalSecret, preview.verification_phrase, csrf); setPairingState("approved"); }
    catch (error) { setPairingState("terminal"); setMessage(errorMessage(error)); } finally { setBusy(false); }
  };

  const refreshAccount = async () => { const session = await api.browserSession(); setCsrf(session.csrf_token); setAuthenticatedAccountName(session.account_display_name); setAccount(await api.browserAccount()); };
  const rename = async (deviceId: string, oldName: string) => {
    const name = window.prompt("Новое имя устройства", oldName)?.trim();
    if (!name || name === oldName) return;
    setBusy(true); setMessage("");
    try { await api.renameDevice(deviceId, name, csrf); await refreshAccount(); setMessage("Имя устройства обновлено."); } catch (error) { setMessage(errorMessage(error)); } finally { setBusy(false); }
  };
  const revoke = async (deviceId: string, name: string) => {
    if (!window.confirm(`Отключить «${name}»? На нём будет завершён вход в RockCast или RockMobile. Это не завершит вход в текущем браузере.`)) return;
    setBusy(true); setMessage("");
    try { await api.revokeDevice(deviceId, csrf); await refreshAccount(); setMessage("Устройство отключено."); } catch (error) { setMessage(errorMessage(error)); } finally { setBusy(false); }
  };
  const logout = async () => {
    if (!window.confirm("Выйти из Rock-аккаунта в этом браузере? Устройства останутся подключёнными.")) return;
    setBusy(true); setMessage("");
    try { await api.logoutBrowser(csrf); setCsrf(""); setAuthenticatedAccountName(""); setAccount(undefined); setMessage("Вы вышли из аккаунта в этом браузере."); } catch (error) { setMessage(errorMessage(error)); } finally { setBusy(false); }
  };

  if (!isPairing) return <main><header><span>ROCK</span><h1>Rock-аккаунт</h1></header><section><h2>{authenticatedAccountName ? `Аккаунт «${authenticatedAccountName}»` : "Вы не вошли"}</h2><p>{authenticatedAccountName ? "RockCast и RockMobile, показанные ниже, принадлежат этому аккаунту." : "Войдите, чтобы увидеть подключённые устройства или подтвердить устройство по защищённой ссылке."}</p>{!authenticatedAccountName && <button onClick={authenticate} disabled={busy}>{busy ? "Проверяем…" : "Войти с passkey"}</button>}{authenticatedAccountName && <button className="secondary" onClick={logout} disabled={busy}>Выйти из браузера</button>}</section>{account && <><section><h2>Устройства ({account.devices.length} из {account.device_limit})</h2><p>Чтобы подключить новое устройство после достижения лимита, отключите здесь ненужное устройство. Управление системными passkey выполняется в настройках браузера или ОС.</p>{account.devices.length === 0 ? <p>Пока нет подключённых устройств.</p> : <ul className="devices">{account.devices.map(device => <li key={device.device_id}><div><strong>{deviceProductName(device.device_type)} — {device.device_display_name}</strong><p>{device.session_status === "active" ? "Сессия активна" : "Нет активной сессии"} · Подключено {formatDate(device.connected_at)}{device.last_seen_at && ` · Активность ${formatDate(device.last_seen_at)}`}</p></div><div><button className="secondary" onClick={() => rename(device.device_id, device.device_display_name)} disabled={busy}>Переименовать</button><button className="danger" onClick={() => revoke(device.device_id, device.device_display_name)} disabled={busy}>Отключить</button></div></li>)}</ul>}</section><section><h2>Безопасность доступа</h2><p>«Отключить» завершает native-сессии выбранного RockCast или RockMobile. Это не завершает вход в текущем браузере; для него используйте «Выйти из браузера» выше.</p><p>Сервер отключает устройства и сессии, но не удаляет passkey из браузера или Google Password Manager. Старый ключ удаляйте вручную только после успешного входа новым ключом. Одинаковое имя «RockServer user» само по себе не доказывает, что запись старая.</p></section></>}{message && <p role="alert">{message}</p>}<footer>Passkey и данные сессии не сохраняются в браузере.</footer></main>;

  const deviceType = deviceProductName(preview?.device_type ?? "");
  return <main><header><span>ROCK</span><h1>Подключение устройства</h1></header>
    {preview && pairingState !== "approved" && <section><p className="eyebrow">Проверьте, что это ваше устройство</p><h2>{deviceType} — {preview.device_display_name}</h2><dl><div><dt>Проверочная фраза</dt><dd>{preview.verification_phrase}</dd></div><div><dt>Короткий код</dt><dd>{preview.short_code}</dd></div><div><dt>Действует до</dt><dd>{new Intl.DateTimeFormat("ru-RU", { dateStyle: "medium", timeStyle: "short" }).format(new Date(preview.expires_at))}</dd></div></dl></section>}
    {message && <p role="alert">{message}</p>}
    {preview && pairingState === "approved" ? <section><p className="eyebrow">Устройство подключено</p><h2>{deviceType} — {preview.device_display_name} подключён к «{authenticatedAccountName}»</h2>{deviceType === "RockMobile" ? <a className="button" href="/return/rockmobile">Вернуться в RockMobile</a> : <p>Вернитесь в RockCast.</p>}<a className="button secondary" href="/">Открыть аккаунт и устройства</a></section>
      : preview && authenticatedAccountName && csrf ? <section><h2>Подключить {deviceType} к аккаунту «{authenticatedAccountName}»?</h2><p>Будет подключено только показанное выше устройство.</p><button onClick={approve} disabled={pairingState !== "authenticated"}>{pairingState === "approving" ? "Подключаем…" : "Подключить"}</button><a className="button secondary" href="/">Отмена</a></section>
      : preview ? <section><h2>Чтобы продолжить</h2><p>Войдите в существующий Rock-аккаунт или создайте новый. После этого вы вернётесь к этому устройству.</p><button onClick={authenticate} disabled={busy}>{busy ? "Проверяем…" : "Войти с passkey"}</button><div className="create"><h3>Создать Rock-аккаунт</h3><p>Это создаст новый отдельный аккаунт, а не войдёт в существующий.</p><label>Имя аккаунта <input value={registrationName} maxLength={128} placeholder="Например, Алексей" onInput={event => setRegistrationName(event.currentTarget.value)} /></label><button className="secondary" onClick={register} disabled={busy}>Создать Rock-аккаунт</button></div></section>
      : <section><h2>Ссылка подключения недействительна</h2><p>Откройте новую защищённую ссылку на устройстве, которое хотите подключить.</p></section>}
    <footer>Passkey и данные сессии не сохраняются в браузере.</footer></main>;
}
