//! End-to-end integration tests for the tracking/enrich adapters.

use om_core::model::IdSet;
use om_core::ports::{Enricher, Tracker};
use om_core::tracking::ListStatus;
use om_track::{AniListTracker, AniSkipEnricher, MalTracker};
use serde_json::json;
use wiremock::matchers::{body_string_contains, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn aniskip_parses_op_and_ed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/skip-times/52991/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "found": true,
            "results": [
                { "interval": { "startTime": 84.5, "endTime": 174.2 }, "skipType": "op" },
                { "interval": { "startTime": 1320.0, "endTime": 1410.0 }, "skipType": "ed" }
            ]
        })))
        .mount(&server)
        .await;

    let enricher = AniSkipEnricher::with_bases(server.uri(), server.uri());
    let skip = enricher
        .skip_times(&IdSet::default().with_mal(52991), 1, None)
        .await
        .unwrap();

    let op = skip.opening.unwrap();
    assert_eq!(op.start, 85); // rounded from 84.5
    assert_eq!(op.end, 174);
    assert_eq!(skip.ending.unwrap().start, 1320);
}

/// When the caller knows the episode runtime, it must be forwarded to AniSkip as
/// `episodeLength` (seconds) so the API can validate intervals against the
/// episode length — rather than the `0` sentinel that disables that check.
#[tokio::test]
async fn aniskip_sends_real_episode_length_when_known() {
    let server = MockServer::start().await;
    // The mock only responds when `episodeLength=1440` (24 min × 60) is present;
    // a request still hardcoding `0` would 404 and fail the assertions below.
    Mock::given(method("GET"))
        .and(path("/v1/skip-times/52991/1"))
        .and(query_param("episodeLength", "1440"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "found": true,
            "results": [
                { "interval": { "startTime": 0.0, "endTime": 90.0 }, "skipType": "op" }
            ]
        })))
        .mount(&server)
        .await;

    let enricher = AniSkipEnricher::with_bases(server.uri(), server.uri());
    let skip = enricher
        .skip_times(&IdSet::default().with_mal(52991), 1, Some(1440))
        .await
        .unwrap();

    let op = skip.opening.expect("opening interval present");
    assert_eq!(op.start, 0);
    assert_eq!(op.end, 90);
}

#[tokio::test]
async fn aniskip_not_found_is_empty_not_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let enricher = AniSkipEnricher::with_bases(server.uri(), server.uri());
    let skip = enricher
        .skip_times(&IdSet::default().with_mal(1), 1, None)
        .await
        .unwrap();
    assert!(skip.is_empty());
}

#[tokio::test]
async fn aniskip_requires_mal_id() {
    let enricher = AniSkipEnricher::with_bases("http://127.0.0.1:1", "http://127.0.0.1:1");
    assert!(enricher
        .skip_times(&IdSet::default().with_anilist(5), 1, None)
        .await
        .is_err());
}

#[tokio::test]
async fn jikan_collects_filler_episodes() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v4/anime/52991/episodes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "pagination": { "last_visible_page": 1, "has_next_page": false },
            "data": [
                { "mal_id": 1, "filler": false },
                { "mal_id": 2, "filler": true },
                { "mal_id": 3, "filler": false },
                { "mal_id": 4, "filler": true }
            ]
        })))
        .mount(&server)
        .await;

    let enricher = AniSkipEnricher::with_bases(server.uri(), server.uri());
    let filler = enricher
        .filler_episodes(&IdSet::default().with_mal(52991))
        .await
        .unwrap();
    assert_eq!(filler, vec![2, 4]);
}

#[tokio::test]
async fn anilist_tracker_updates_progress() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_string_contains("SaveMediaListEntry"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "SaveMediaListEntry": { "id": 1, "progress": 5, "status": "CURRENT" } }
        })))
        .mount(&server)
        .await;

    let tracker = AniListTracker::with_base_url("tok", server.uri());
    tracker
        .update_progress(&IdSet::default().with_anilist(154587), 5)
        .await
        .unwrap();
    tracker
        .set_status(
            &IdSet::default().with_anilist(154587),
            ListStatus::Completed,
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn anilist_tracker_surfaces_graphql_errors() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "errors": [{ "message": "Invalid token" }]
        })))
        .mount(&server)
        .await;
    let tracker = AniListTracker::with_base_url("bad", server.uri());
    let err = tracker
        .update_progress(&IdSet::default().with_anilist(1), 1)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("Invalid token"));
}

#[tokio::test]
async fn mal_tracker_updates_and_handles_auth() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/anime/52991/my_list_status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "num_episodes_watched": 5, "status": "watching"
        })))
        .mount(&server)
        .await;

    let tracker = MalTracker::with_base_url("tok", server.uri());
    tracker
        .update_progress(&IdSet::default().with_mal(52991), 5)
        .await
        .unwrap();
    tracker
        .rate(&IdSet::default().with_mal(52991), 8.6)
        .await
        .unwrap();
}

#[tokio::test]
async fn mal_tracker_maps_401_to_auth_error() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let tracker = MalTracker::with_base_url("bad", server.uri());
    let err = tracker
        .update_progress(&IdSet::default().with_mal(1), 1)
        .await
        .unwrap_err();
    assert!(matches!(err, om_core::error::CoreError::Auth(_)));
}
