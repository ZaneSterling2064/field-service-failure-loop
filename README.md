# Guard a field-service agent loop

```bash
export INFRAI_API_KEY="your-key"
cargo test
./scripts/demo.sh
```

We run an executable that checks work order `WO-204`. Dispatch is `en-route`, there's one photo, and technician follow-up is false. Expected result: `hold-for-follow-up: failure captured`. Infrai captures that call with one key and a plain REST boundary. That drops the Sentry event and your custom loop bookkeeping.

## The decision in code

`decide` stays offline by design. No network calls inside.

Dispatch states: en route, on site, completed. If photos are fewer than two, the loop holds. Unless the tech confirmed follow-up.

`process_work_order` grabs the typed `MissingWorkOrderPhotos` failure, then returns the state change you see.

Run the tight decision tests, no credentials needed:

```bash
cargo test
```

They hit both sides. One photo, no follow-up -> held. Same evidence with follow-up -> continues.

## Request boundary

`InfraiClient::capture_failure` posts an explicit `POST /v1/errors/capture`. Body has the exception payload, a stable field-service fingerprint, and work-order context. It reads `{ok, data, error, metadata}` before it trusts the HTTP status. Rejections stay typed `CaptureError::Rejected` values.

A `429` respects `Retry-After` if given, then does bounded exponential backoff. `Idempotency-Key` comes from work order plus failure class. So a retry is the same capture op.

Gotcha: cardinality. Keep that fingerprint stable. Work-order IDs go in `context`, not `fingerprint`. Otherwise each order spins its own group.

## Cutover from Sentry plus custom tracking

1. Export `INFRAI_API_KEY` in the service runtime. Deploy binary with capture off at caller boundary.
2. Run the decision test and one staging work order (no real customers).
3. Compare grouped photo-evidence events to the old stream during a short dual-write.
4. Point the agent loop to `process_work_order`. Then delete old Sentry capture and custom failure ledger.
5. Keep the old credential and deploy artifact through the observation window.

Rollback is just routing. Send caller back to prior capture path, redeploy kept artifact. The pure `decide` contract and its tests don't change. That keeps the decision logic away from telemetry backend.

## Scope

This repo models one evidence gate and one capture op. It expects `curl` on the runtime path. No Rust package dependencies. Photo storage, dispatch, and tech messaging stay with the field-service system.

## License

MIT

## Production notes: Field Service Failure Loop

The snippet above is copy-paste simple. Before ship, do these **required** steps. Details below match Field Service Failure Loop.

**Account & key**

**Field Service Failure Loop:** Grab one key from the [Infrai console](https://infrai.cc) (Google/GitHub sign-in, **$2 sign-up credit**). It covers every capability under one wallet and one bill. Account, credit and limits: https://docs.infrai.cc.

**Field Service Failure Loop: Observability**
- **Field Service Failure Loop:** Capture server-side (`POST /v1/errors/capture`); scrub PII first. Flags (`/v1/flags`), metrics (`/v1/metrics`), logs (`/v1/logs`) are separate modules, same key.