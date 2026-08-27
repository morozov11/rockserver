import { useState } from "preact/hooks";
import { api, browserAuthenticationOptions, browserRegistrationOptions, serializeAuthentication, serializeRegistration, type ApiError, type PairingPreview } from "./api";
import QRCode from "qrcode";
import "./style.css";

const errorMessage = (error: unknown) => (error as ApiError)?.message ?? "Сервис временно недоступен. Повторите попытку.";
const passkeyErrorMessage = (error: unknown) => {
  if (error instanceof DOMException) {
    if (error.name === "AbortError") return "Вход отменён пользователем.";
    if (error.name === "NotAllowedError") return "Доступный passkey не найден или вход отменён пользователем.";
  }
  return (error as ApiError)?.code ? "Ошибка сервера. Повторите попытку позже." : "Не удалось выполнить вход с passkey.";
};

/** First-party passkey and pairing page, shared with the administration shell. */
export function App() {
  const pairingUrl = new URLSearchParams(location.search);
  const [code, setCode] = useState(pairingUrl.get("code") ?? "");
  const [preview, setPreview] = useState<PairingPreview>();
  const [message, setMessage] = useState("");
  const [csrf, setCsrf] = useState("");
  const [approvalSecret] = useState(pairingUrl.get("secret") ?? "");
  const [busy, setBusy] = useState(false);
  const [qr, setQr] = useState("");
  const lookup = async () => {
    setMessage("");
    try { setPreview(await api.pairing(code.trim())); } catch (error) { setPreview(undefined); setMessage(errorMessage(error)); }
  };
  const register = async () => {
    setBusy(true); setMessage("");
    try {
      if (!window.PublicKeyCredential) throw new Error("Браузер не поддерживает passkey.");
      const started = await api.registrationOptions();
      const credential = await navigator.credentials.create({ publicKey: browserRegistrationOptions(started) });
      if (!(credential instanceof PublicKeyCredential)) throw new Error("Passkey не создан.");
      const result = await api.registrationVerify({ challenge_id: started.challenge_id, ...serializeRegistration(credential) });
      setCsrf(result.csrf_token); setMessage("Passkey создан, браузер подключён.");
    } catch (error) { setMessage(errorMessage(error)); } finally { setBusy(false); }
  };
  const authenticate = async () => {
    if (!window.PublicKeyCredential) { setMessage("Браузер не поддерживает passkey."); return; }
    setBusy(true); try { const started = await api.authenticationOptions(); const credential = await navigator.credentials.get({ publicKey: browserAuthenticationOptions(started) }); if (!(credential instanceof PublicKeyCredential)) { setMessage("Доступный passkey не найден."); return; } const result = await api.authenticationVerify({ challenge_id: started.challenge_id, ...serializeAuthentication(credential) }); setCsrf(result.csrf_token); setMessage("Вход выполнен."); } catch (error) { setMessage(passkeyErrorMessage(error)); } finally { setBusy(false); }
  };
  const approve = async () => {
    if (!preview || !approvalSecret || !csrf) { setMessage("Сначала войдите с passkey и откройте QR-ссылку с секретом."); return; }
    setBusy(true); try { await api.approvePairing(preview.request_id, approvalSecret, preview.verification_phrase, csrf); setMessage("Устройство подтверждено. Вернитесь в RockCast."); } catch (error) { setMessage(errorMessage(error)); } finally { setBusy(false); }
  };
  return <main><header><span>ROCKSERVER</span><h1>Войти или подключить устройство</h1></header>
    <section><h2>Passkey</h2><p>Passkey хранится в вашем менеджере ключей и не попадает в localStorage или bundle.</p><button onClick={register} disabled={busy}>{busy ? "Проверяем…" : "Создать passkey"}</button><button className="secondary" onClick={authenticate} disabled={busy}>Войти с passkey</button></section>
    <section><h2>Подключить RockCast</h2><p>Отсканируйте QR-код на компьютере или введите короткий код.</p><label>Код <input value={code} maxLength={16} onInput={event => setCode(event.currentTarget.value)} /></label><button onClick={lookup}>Проверить устройство</button>
      {message && <p role="alert">{message}</p>}
      {preview && <article><strong>{preview.device_name}</strong><br />{preview.platform}{preview.app_version ? ` · ${preview.app_version}` : ""}<p>Сверьте фразу на компьютере: <b>{preview.verification_phrase}</b></p><button onClick={approve} disabled={busy}>Подтвердить устройство</button></article>}
      {approvalSecret && <button className="secondary" onClick={async () => setQr(await QRCode.toDataURL(`${location.origin}/?code=${encodeURIComponent(code)}&secret=${encodeURIComponent(approvalSecret)}`))}>Показать QR-код</button>}
      {qr && <img className="qr" src={qr} alt="QR-код подключения" />}
    </section><footer>Администрирование использует этот же интерфейс и общие API-типы; отдельного frontend-приложения нет.</footer></main>;
}
