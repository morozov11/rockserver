//! Liveness, readiness, and the local administration preview.

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

use super::state::AppState;

/// Stable JSON response returned by the health endpoints.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HealthResponse {
    /// Current process health status.
    pub status: HealthStatus,
}

/// Operational health states returned by liveness and readiness endpoints.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    /// The process is running or its configured dependencies are available.
    Ok,
    /// A required dependency is currently unavailable.
    NotReady,
}

const ADMIN_CONSOLE_HTML: &str = r#"<!doctype html>
<html lang="ru">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>RockServer — администрирование</title>
  <style>
    :root { color-scheme: dark; font-family: Inter, system-ui, sans-serif; background: #10151b; color: #edf3f8; }
    body { max-width: 1040px; margin: 0 auto; padding: 42px 24px 72px; }
    header { display: flex; align-items: center; justify-content: space-between; gap: 20px; margin-bottom: 32px; }
    h1 { font-size: clamp(1.7rem, 4vw, 2.5rem); margin: 0; } h2 { margin: 0 0 14px; font-size: 1.1rem; }
    .brand { color: #77d4ff; font-weight: 800; letter-spacing: .08em; font-size: .82rem; }
    .notice, .panel { background: #18212b; border: 1px solid #2c3d4d; border-radius: 14px; }
    .notice { padding: 14px 16px; color: #b9c9d7; margin-bottom: 22px; }
    .grid { display: grid; grid-template-columns: repeat(3, 1fr); gap: 16px; margin-bottom: 22px; }
    .panel { padding: 20px; } .metric { font-size: 1.8rem; font-weight: 750; color: #77d4ff; }
    .muted { color: #a8b8c6; font-size: .92rem; } label { display: block; margin: 12px 0 7px; font-weight: 650; }
    input { box-sizing: border-box; width: 100%; padding: 12px; border: 1px solid #405464; border-radius: 8px; background: #0e141a; color: inherit; }
    button { border: 0; border-radius: 8px; padding: 12px 16px; background: #43b8ed; color: #06131a; font-weight: 750; cursor: pointer; }
    button.secondary { background: #2a3946; color: #e8f2f8; } .actions { display: flex; gap: 10px; margin-top: 14px; }
    #workspace { display: none; } #message { min-height: 1.3em; margin-top: 12px; } .error { color: #ff9b9b; } .ok { color: #9de6ad; }
    table { width: 100%; border-collapse: collapse; margin-top: 14px; } th, td { text-align: left; padding: 11px 8px; border-bottom: 1px solid #2b3b49; vertical-align: top; }
    th { color: #a8c2d3; font-size: .8rem; text-transform: uppercase; } a { color: #81d8ff; } code { color: #bed5e5; }
    @media (max-width: 680px) { body { padding: 26px 16px; } header { display: block; } .grid { grid-template-columns: 1fr; } table { font-size: .85rem; } }
  </style>
</head>
<body>
  <header><div><div class="brand">ROCKSERVER</div><h1>Панель администратора</h1></div><span class="muted">локальный предпросмотр</span></header>
  <p class="notice">Это read-only просмотр: токен не сохраняется в браузере и используется только для запросов в этой вкладке. Очистка аккаунтов выполняется отдельной операторской командой с preview и точным подтверждением; эта страница не удаляет пользователей, passkey или устройства.</p>
  <section id="login" class="panel"><h2>Подключить консоль</h2><p class="muted">Введите значение <code>ROCKSERVER_API_BEARER_TOKEN</code> текущего сервера.</p><label for="token">Bearer token</label><input id="token" type="password" autocomplete="off" placeholder="Токен из переменной окружения"><div class="actions"><button id="connect">Подключиться</button></div><div id="message" aria-live="polite"></div></section>
  <main id="workspace">
    <section class="grid"><article class="panel"><h2>Сервис</h2><div class="metric" id="ready">—</div><span class="muted">готовность каталога</span></article><article class="panel"><h2>Доступ</h2><div class="metric">Bearer</div><span class="muted">токен только в памяти</span></article><article class="panel"><h2>Каталог</h2><div class="metric" id="result-count">—</div><span class="muted">найдено последним запросом</span></article></section>
    <section class="panel"><h2>Поиск по станциям</h2><label for="query">Запрос</label><input id="query" value="rock" maxlength="500"><div class="actions"><button id="search">Найти станции</button><button id="disconnect" class="secondary">Отключиться</button></div><div id="search-message" aria-live="polite"></div><table><thead><tr><th>Станция</th><th>Теги</th><th>Страна</th><th>Поток</th></tr></thead><tbody id="stations"><tr><td colspan="4" class="muted">Введите запрос и нажмите «Найти станции».</td></tr></tbody></table></section>
  </main>
  <script>
    let token = '';
    const $ = id => document.getElementById(id);
    const setMessage = (id, text, error = false) => { const node = $(id); node.textContent = text; node.className = error ? 'error' : 'ok'; };
    const escape = value => String(value).replace(/[&<>'"]/g, char => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', "'": '&#39;', '"': '&quot;' })[char]);
    async function api(path, options = {}) { const headers = new Headers(options.headers || {}); headers.set('Authorization', `Bearer ${token}`); return fetch(path, { ...options, headers }); }
    async function readiness() { const response = await fetch('/health/ready'); $('ready').textContent = response.ok ? 'Готов' : 'Недоступен'; }
    async function runSearch() {
      const query = $('query').value.trim(); if (!query) { setMessage('search-message', 'Введите поисковый запрос.', true); return; }
      const response = await api('/v1/search', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ query, locale: 'en-US', limit: 20 }) });
      if (!response.ok) { setMessage('search-message', response.status === 401 ? 'Токен не принят сервером.' : 'Не удалось выполнить поиск.', true); return; }
      const data = await response.json(); const stations = data.stations || []; $('result-count').textContent = stations.length;
      $('stations').innerHTML = stations.length ? stations.map(station => `<tr><td><strong>${escape(station.name)}</strong><br><span class="muted">${escape(station.id)}</span></td><td>${escape(station.tags.join(', '))}</td><td>${escape(station.country_code || '—')}</td><td><a href="${escape(station.stream_url)}" target="_blank" rel="noopener">открыть</a></td></tr>`).join('') : '<tr><td colspan="4" class="muted">Станции не найдены.</td></tr>';
      setMessage('search-message', `Запрос обработан: ${data.request_id}`);
    }
    $('connect').addEventListener('click', async () => { token = $('token').value; if (!token) { setMessage('message', 'Введите токен.', true); return; } const response = await api('/v1/search', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ query: 'rock', limit: 1 }) }); if (!response.ok) { const unauthorized = response.status === 401; setMessage('message', unauthorized ? 'Токен не принят сервером.' : `Сервер ответил HTTP ${response.status}; токен не проверен.`, true); if (unauthorized) token = ''; return; } $('login').style.display = 'none'; $('workspace').style.display = 'block'; await readiness(); runSearch(); });
    $('search').addEventListener('click', runSearch); $('query').addEventListener('keydown', event => { if (event.key === 'Enter') runSearch(); });
    $('disconnect').addEventListener('click', () => { token = ''; $('token').value = ''; $('workspace').style.display = 'none'; $('login').style.display = 'block'; setMessage('message', 'Токен очищен из памяти вкладки.'); });
  </script>
</body>
</html>"#;

fn health_response() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: HealthStatus::Ok,
    })
}

/// Serves the local administration-console preview.
pub(super) async fn admin_console() -> Html<&'static str> {
    Html(ADMIN_CONSOLE_HTML)
}

/// Returns the process liveness response.
pub(super) async fn live() -> Json<HealthResponse> {
    health_response()
}

/// Returns readiness based on the search-service dependency.
pub(super) async fn ready(State(state): State<AppState>) -> Response {
    match state.search_service.check_readiness().await {
        Ok(()) => health_response().into_response(),
        Err(error) => {
            tracing::warn!(error = %error, "readiness dependency check failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(HealthResponse {
                    status: HealthStatus::NotReady,
                }),
            )
                .into_response()
        }
    }
}
