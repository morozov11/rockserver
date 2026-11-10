use std::time::{Duration, Instant};

use super::{PublicLimit, VoiceRateBucket};

#[test]
fn voice_command_refills_one_token_every_five_seconds() {
    let limit = PublicLimit {
        requests: 12,
        burst: 4,
    };
    let start = Instant::now();
    let mut bucket = VoiceRateBucket {
        tokens: limit.burst as f64,
        updated_at: start,
    };
    for _ in 0..4 {
        assert_eq!(bucket.consume(limit, start), Ok(()));
    }
    assert_eq!(bucket.consume(limit, start), Err(5));
    assert_eq!(
        bucket.consume(limit, start + Duration::from_secs(1)),
        Err(4)
    );
    assert_eq!(
        bucket.consume(limit, start + Duration::from_secs(5)),
        Ok(())
    );
    assert_eq!(
        bucket.consume(limit, start + Duration::from_secs(5)),
        Err(5)
    );
    assert_eq!(
        bucket.consume(limit, start + Duration::from_secs(60)),
        Ok(())
    );
    for _ in 0..3 {
        assert_eq!(
            bucket.consume(limit, start + Duration::from_secs(60)),
            Ok(())
        );
    }
    assert_eq!(
        bucket.consume(limit, start + Duration::from_secs(60)),
        Err(5)
    );
}

#[test]
fn voice_stream_refills_one_token_every_ten_seconds() {
    let limit = PublicLimit {
        requests: 6,
        burst: 2,
    };
    let start = Instant::now();
    let mut bucket = VoiceRateBucket {
        tokens: limit.burst as f64,
        updated_at: start,
    };
    assert_eq!(bucket.consume(limit, start), Ok(()));
    assert_eq!(bucket.consume(limit, start), Ok(()));
    assert_eq!(bucket.consume(limit, start), Err(10));
    assert_eq!(
        bucket.consume(limit, start + Duration::from_secs(9)),
        Err(1)
    );
    assert_eq!(
        bucket.consume(limit, start + Duration::from_secs(10)),
        Ok(())
    );
    assert_eq!(
        bucket.consume(limit, start + Duration::from_secs(10)),
        Err(10)
    );
}
