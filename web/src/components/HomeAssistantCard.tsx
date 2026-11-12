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

/** Shows only the signed-in owner's approved HA readings and their freshness. */
export function HomeAssistantCard() {
  const [data, setData] = useState<HaOverview | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(false);

  const load = async () => {
    setLoading(true);
    setError(false);
    try { setData(await api.homeAssistant()); }
    catch { setError(true); }
    finally { setLoading(false); }
  };
  useEffect(() => { void load(); }, []);

  const label = loading ? "Загрузка" : error ? "Ошибка" : !data?.connected ? "Не подключён" : data.online ? "На связи" : "Нет связи";
  return (
    <section class="deck-panel yandex-home-deck" aria-label="Home Assistant">
      <div class="panel-header-row">
        <div class="yh-heading"><p class="eyebrow">Локальный дом</p><h2>Home Assistant</h2></div>
        <span class={`yh-state-badge ${!data?.online ? "yh-state-disconnected" : ""}`}>{label}</span>
      </div>
      <div class="yh-body" aria-live="polite">
        {loading ? <p class="yh-hint" role="status">Обновляем данные…</p>
          : error ? <p class="yh-hint" role="alert">Не удалось получить данные Home Assistant.</p>
          : !data?.connected ? <p class="yh-hint">Интеграция пока не настроена. Для подключения нужен локальный компонент HA и учётные данные RockServer.</p>
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
      </div>
      <div class="yandex-actions-row"><button type="button" class="secondary" onClick={() => void load()} disabled={loading}>Обновить показания</button></div>
    </section>
  );
}
