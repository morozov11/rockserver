import { useEffect, useState } from "preact/hooks";
import { api, type HaOverview, type HaWeather } from "../api";
import { formatDate } from "./HardwareHud";

const WEATHER: Record<string, string> = {
  "clear-night": "Ясная ночь", cloudy: "Облачно", fog: "Туман", hail: "Град",
  lightning: "Гроза", "lightning-rainy": "Гроза с дождём", partlycloudy: "Переменная облачность",
  pouring: "Ливень", rainy: "Дождь", snowy: "Снег", "snowy-rainy": "Мокрый снег",
  sunny: "Солнечно", windy: "Ветрено", "windy-variant": "Ветрено",
};

const condition = (value: string | null) => value ? WEATHER[value] ?? value : "Нет данных";
const measure = (value: number | null, unit: string | null) => value === null ? "Нет данных" : `${Number(value.toFixed(1))}${unit ? ` ${unit}` : ""}`;
const freshness = (status: "fresh" | "stale" | "unavailable") => status === "fresh" ? "Актуально" : status === "stale" ? "Устарело" : "Недоступно";

function WeatherDetails({ weather }: { weather: HaWeather }) {
  return (
    <div class="ha-weather">
      <h3>Погода сейчас <small>{freshness(weather.status)}</small></h3>
      {weather.status === "unavailable" ? <p class="yh-hint">Текущая погода недоступна.</p> : (
        <div class="sensor-properties">
          <div class="sensor-property-row"><span>Условия</span><strong>{condition(weather.condition)}</strong></div>
          <div class="sensor-property-row"><span>Температура</span><strong>{measure(weather.temperature, weather.temperature_unit)}</strong></div>
          {weather.humidity !== null && <div class="sensor-property-row"><span>Влажность</span><strong>{measure(weather.humidity, "%")}</strong></div>}
          {weather.wind_speed !== null && <div class="sensor-property-row"><span>Ветер</span><strong>{measure(weather.wind_speed, weather.wind_speed_unit)}</strong></div>}
          <small>Обновлено {formatDate(weather.observed_at)}{weather.status === "stale" ? " · последние известные данные" : ""}</small>
        </div>
      )}
      <h3>Краткий прогноз <small>{freshness(weather.forecast_status)}</small></h3>
      {weather.forecast_status === "unavailable" ? <p class="yh-hint">Прогноз недоступен.</p> : (
        <>
          {weather.forecast_status === "stale" && <p class="yh-hint">Последний прогноз устарел.</p>}
          <ul class="ha-forecast">
            {weather.forecast.slice(0, 3).map((day) => (
              <li key={day.datetime}>
                <span>{new Intl.DateTimeFormat("ru-RU", { day: "numeric", month: "short" }).format(new Date(day.datetime))}</span>
                <span>{condition(day.condition)}</span>
                <strong>{measure(day.temperature, weather.temperature_unit)}{day.templow !== null ? ` / ${measure(day.templow, weather.temperature_unit)}` : ""}</strong>
              </li>
            ))}
          </ul>
        </>
      )}
    </div>
  );
}

/** Shows the owner's HA readings and browser-managed connection settings. */
export function HomeAssistantCard({ csrf }: { csrf: string }) {
  const [data, setData] = useState<HaOverview | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(false);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [credential, setCredential] = useState<string | null>(null);
  const [sensorIds, setSensorIds] = useState("");
  const [weatherId, setWeatherId] = useState("");

  const load = async () => {
    setLoading(true);
    setError(false);
    try {
      const overview = await api.homeAssistant();
      setData(overview);
      setSensorIds(overview.allowed_sensors.join("\n"));
      setWeatherId(overview.weather_entity ?? "");
    }
    catch { setError(true); }
    finally { setLoading(false); }
  };
  useEffect(() => { void load(); }, []);

  const issue = async () => {
    if (data?.connected && !window.confirm("Заменить ключ Home Assistant? Старый ключ сразу перестанет работать, прежние показания будут очищены.")) return;
    setBusy(true);
    setMessage("");
    setCredential(null);
    try {
      const sensors = sensorIds.split(/[,\n]/).map((id) => id.trim()).filter(Boolean);
      const response = await api.issueHomeAssistantCredential({ sensors, weather_entity: weatherId.trim() }, csrf);
      setCredential(response.credential);
      await load();
    } catch { setMessage("Не удалось выдать ключ. Проверьте ID датчиков и погоды."); }
    finally { setBusy(false); }
  };

  const revoke = async () => {
    if (!window.confirm("Отключить Home Assistant? Ключ будет отозван, показания исчезнут из кабинета.")) return;
    setBusy(true);
    setMessage("");
    setCredential(null);
    try { await api.revokeHomeAssistant(csrf); await load(); }
    catch { setMessage("Не удалось отключить Home Assistant."); }
    finally { setBusy(false); }
  };

  const label = loading ? "Загрузка" : error ? "Ошибка" : !data?.connected ? "Не подключён" : data.online ? "На связи" : "Нет связи";
  const serverUrl = `wss://${location.host}/api/v1/integrations/home-assistant/connect`;
  return (
    <section class="deck-panel yandex-home-deck" aria-label="Home Assistant">
      <div class="panel-header-row">
        <div class="yh-heading"><p class="eyebrow">Локальный дом</p><h2>Home Assistant</h2></div>
        <span class={`yh-state-badge ${!data?.online ? "yh-state-disconnected" : ""}`}>{label}</span>
      </div>
      <div class="yh-body" aria-live="polite">
        {loading ? <p class="yh-hint" role="status">Обновляем данные…</p>
          : error ? <p class="yh-hint" role="alert">Не удалось получить данные Home Assistant.</p>
          : !data?.connected ? <p class="yh-hint">Установите компонент RockServer Bridge в HA один раз. Здесь выберите сущности и получите ключ, затем вставьте его в настройках интеграции HA.</p>
          : <>
              {!data.online && <p class="yh-hint">Нет связи с Home Assistant. Последние показания помечены как устаревшие.</p>}
              <h3>Датчики</h3>
              {data.sensors.length ? <ul class="sensors" aria-label="Датчики Home Assistant">
                {data.sensors.map((sensor) => <li key={sensor.entity_id} class="sensor-card">
                  <div class="sensor-property-row"><strong>{sensor.display_name}</strong><span>{freshness(sensor.status)}</span></div>
                  <div class="sensor-property-row"><span>{sensor.status === "unavailable" ? "Нет показаний" : measure(sensor.value, sensor.unit)}</span>
                    <small>{sensor.status === "stale" ? "Последнее: " : "Обновлено: "}{formatDate(sensor.observed_at)}</small></div>
                </li>)}
              </ul> : <p class="yh-hint">Разрешённые датчики ещё не прислали показания.</p>}
              {data.weather ? <WeatherDetails weather={data.weather} /> : <p class="yh-hint">Погодные данные ещё не получены.</p>}
            </>}
        <div class="ha-settings">
          <h3>Подключение и доступ</h3>
          <label>Датчики HA, по одному ID в строке
            <textarea value={sensorIds} onInput={(event) => setSensorIds(event.currentTarget.value)} rows={3} placeholder="sensor.room_temperature" disabled={busy} />
          </label>
          <label>Погода HA
            <input value={weatherId} onInput={(event) => setWeatherId(event.currentTarget.value)} placeholder="weather.home" disabled={busy} />
          </label>
          <p class="yh-hint">ID сущностей можно скопировать в HA: Инструменты разработчика → Состояния. В HA укажите адрес <code>{serverUrl}</code>.</p>
          {credential && <div class="ha-credential" role="status"><p>Новый ключ доступа. Скопируйте его сейчас: после закрытия страницы он больше не показывается.</p><input readOnly value={credential} aria-label="Ключ доступа RockServer для HA" /><button type="button" class="secondary" onClick={() => void navigator.clipboard.writeText(credential)}>Скопировать ключ</button><button type="button" class="secondary" onClick={() => setCredential(null)}>Скрыть ключ</button></div>}
          {message && <p class="yh-hint" role="alert">{message}</p>}
          <div class="yandex-actions-row"><button type="button" onClick={() => void issue()} disabled={busy || loading}>{data?.connected ? "Заменить ключ и список" : "Получить ключ"}</button>{data?.connected && <button type="button" class="secondary" onClick={() => void revoke()} disabled={busy}>Отключить</button>}</div>
        </div>
      </div>
      <div class="yandex-actions-row"><button type="button" class="secondary" onClick={() => void load()} disabled={loading}>Обновить показания</button></div>
    </section>
  );
}
