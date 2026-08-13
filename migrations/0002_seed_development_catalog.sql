INSERT INTO stations (id, name, homepage_url, tags, language, country_code)
VALUES
    ('station-ambient-001', 'Arctic Ambient', 'https://example.com/arctic-ambient', ARRAY['ambient', 'calm', 'electronic', 'instrumental'], 'en', 'IS'),
    ('station-jazz-001', 'Quiet Jazz Radio', 'https://example.com/quiet-jazz', ARRAY['calm', 'instrumental', 'jazz'], 'en', 'US'),
    ('station-jazz-002', 'Midnight Jazz Lounge', NULL, ARRAY['jazz', 'smooth'], 'en', 'GB'),
    ('station-rock-001', 'Highway Rock', NULL, ARRAY['classic rock', 'rock', 'upbeat'], 'en', 'GB'),
    ('station-rock-002', 'Heritage Rock', 'https://example.com/heritage-rock', ARRAY['classic rock', 'rock'], 'en', 'US'),
    ('station-rock-ru-001', 'Радио Рок', 'https://example.com/radio-rock-ru', ARRAY['classic rock', 'rock'], 'ru', 'RU')
ON CONFLICT (id) DO UPDATE SET
    name = EXCLUDED.name,
    homepage_url = EXCLUDED.homepage_url,
    tags = EXCLUDED.tags,
    language = EXCLUDED.language,
    country_code = EXCLUDED.country_code,
    updated_at = now();

INSERT INTO station_streams (station_id, stream_url, codec, bitrate_kbps, health, is_primary)
VALUES
    ('station-ambient-001', 'https://streams.example.com/arctic-ambient.mp3', 'MP3', 160, 'healthy', true),
    ('station-jazz-001', 'https://streams.example.com/quiet-jazz.mp3', 'MP3', 192, 'healthy', true),
    ('station-jazz-002', 'https://streams.example.com/midnight-jazz.aac', 'AAC', 128, 'healthy', true),
    ('station-rock-001', 'https://streams.example.com/highway-rock.aac', 'AAC', 128, 'healthy', true),
    ('station-rock-002', 'https://streams.example.com/heritage-rock.mp3', 'MP3', 192, 'healthy', true),
    ('station-rock-ru-001', 'https://streams.example.com/radio-rock-ru.mp3', 'MP3', 128, 'healthy', true)
ON CONFLICT (stream_url) DO UPDATE SET
    station_id = EXCLUDED.station_id,
    codec = EXCLUDED.codec,
    bitrate_kbps = EXCLUDED.bitrate_kbps,
    health = EXCLUDED.health,
    is_primary = EXCLUDED.is_primary,
    updated_at = now();
