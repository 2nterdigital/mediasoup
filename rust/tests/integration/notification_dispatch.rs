//! Notification callbacks run on the worker thread. Releasing the last handle of an entity from
//! inside one of them - whether the library does it (the strong `Producer` handles it hands to
//! `on_volumes`/`on_dominant_speaker`) or the application does it from its own callback - tears
//! down that entity's notification subscription, which must not deadlock the worker thread.
//!
//! A deadlocked scenario leaves its own thread and the worker thread parked forever, so every
//! scenario owns all of its entities on a dedicated thread and the test only waits for it with a
//! deadline.

use async_io::Timer;
use futures_lite::future;
use mediasoup::prelude::*;
use mediasoup::producer::DirectProducer;
use mediasoup::worker_manager::WorkerManager;
use mediasoup_types::rtp_parameters::{
    RtpCodecParameters, RtpEncodingParameters, RtpHeaderExtensionParameters,
};
use parking_lot::Mutex;
use std::cell::RefCell;
use std::env;
use std::num::{NonZeroU32, NonZeroU8};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// A scenario that has not finished by then is reported as deadlocked.
const SCENARIO_DEADLINE: Duration = Duration::from_secs(20);
/// How long a scenario waits for the worker to emit the notification it needs.
const NOTIFICATION_DEADLINE: Duration = Duration::from_secs(10);
/// Lowest interval the worker uses for `AudioLevelObserver`, lower values are clamped to it.
const AUDIO_LEVEL_INTERVAL_MS: u16 = 250;
const AUDIO_LEVEL_EXTENSION_ID: u8 = 1;
const RTP_PACKET_INTERVAL: Duration = Duration::from_millis(2);

fn run_scenario<S>(name: &'static str, scenario: S)
where
    S: FnOnce() + Send + 'static,
{
    let (done_tx, done_rx) = mpsc::channel::<()>();

    thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            scenario();
            let _ = done_tx.send(());
        })
        .expect("Failed to spawn scenario thread");

    match done_rx.recv_timeout(SCENARIO_DEADLINE) {
        Ok(()) => {}
        Err(mpsc::RecvTimeoutError::Timeout) => {
            panic!("{name}: did not finish within {SCENARIO_DEADLINE:?}, deadlocked")
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => panic!("{name}: scenario panicked"),
    }
}

/// Reports that the thread it is stored on has exited.
struct ThreadExitSignal(mpsc::Sender<()>);

impl Drop for ThreadExitSignal {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

thread_local! {
    static THREAD_EXIT_SIGNAL: RefCell<Option<ThreadExitSignal>> = const { RefCell::new(None) };
}

fn media_codecs() -> Vec<RtpCodecCapability> {
    vec![RtpCodecCapability::Audio {
        mime_type: MimeTypeAudio::Opus,
        preferred_payload_type: None,
        clock_rate: NonZeroU32::new(48000).unwrap(),
        channels: NonZeroU8::new(2).unwrap(),
        parameters: RtpCodecParametersParameters::default(),
        rtcp_feedback: vec![],
    }]
}

fn audio_producer_options(ssrc: u32) -> ProducerOptions {
    ProducerOptions::new(
        MediaKind::Audio,
        RtpParameters {
            mid: Some(format!("AUDIO-{ssrc}")),
            codecs: vec![RtpCodecParameters::Audio {
                mime_type: MimeTypeAudio::Opus,
                payload_type: 0,
                clock_rate: NonZeroU32::new(48000).unwrap(),
                channels: NonZeroU8::new(2).unwrap(),
                parameters: RtpCodecParametersParameters::default(),
                rtcp_feedback: vec![],
            }],
            header_extensions: vec![RtpHeaderExtensionParameters {
                uri: RtpHeaderExtensionUri::SsrcAudioLevel,
                id: u16::from(AUDIO_LEVEL_EXTENSION_ID),
                encrypt: false,
            }],
            encodings: vec![RtpEncodingParameters {
                ssrc: Some(ssrc),
                ..RtpEncodingParameters::default()
            }],
            ..RtpParameters::default()
        },
    )
}

/// RTP packet with a one-byte `ssrc-audio-level` header extension (RFC 6464) reporting voice
/// activity at 0 dBov, the loudest level, so that `AudioLevelObserver` names the producer in its
/// volumes notification. `ActiveSpeakerObserver` names its only producer as dominant speaker on
/// its first interval even without RTP, that scenario feeds packets only to not depend on it.
fn loud_rtp_packet(sequence_number: u16, timestamp: u32, ssrc: u32) -> Vec<u8> {
    let mut packet = Vec::with_capacity(28);
    // V=2, P=0, X=1, CC=0.
    packet.push(0x90);
    // M=0, PT=0.
    packet.push(0x00);
    packet.extend_from_slice(&sequence_number.to_be_bytes());
    packet.extend_from_slice(&timestamp.to_be_bytes());
    packet.extend_from_slice(&ssrc.to_be_bytes());
    // One-byte header extension (RFC 5285), one 32-bit word long.
    packet.extend_from_slice(&[0xBE, 0xDE, 0x00, 0x01]);
    // Extension id, data length of one byte.
    packet.push(AUDIO_LEVEL_EXTENSION_ID << 4);
    // V=1, level=0.
    packet.push(0x80);
    // Padding up to the 32-bit word.
    packet.extend_from_slice(&[0x00, 0x00]);
    packet.extend_from_slice(&[0xAA; 8]);
    packet
}

struct RtpFeed {
    ssrc: u32,
    sequence_number: u16,
    timestamp: u32,
}

impl RtpFeed {
    fn new(ssrc: u32) -> Self {
        Self {
            ssrc,
            sequence_number: 0,
            timestamp: 0,
        }
    }

    fn send_next(&mut self, producer: &DirectProducer) {
        let _ = producer.send(loud_rtp_packet(
            self.sequence_number,
            self.timestamp,
            self.ssrc,
        ));
        self.sequence_number = self.sequence_number.wrapping_add(1);
        // 20ms of 48kHz audio.
        self.timestamp = self.timestamp.wrapping_add(960);
    }
}

async fn init() -> (Worker, Router, DirectTransport) {
    init_with_settings(WorkerSettings::default()).await
}

/// `WorkerManager` is not returned, which leaves the worker as its last owner.
async fn init_with_settings(worker_settings: WorkerSettings) -> (Worker, Router, DirectTransport) {
    {
        let mut builder = env_logger::builder();
        if env::var(env_logger::DEFAULT_FILTER_ENV).is_err() {
            builder.filter_level(log::LevelFilter::Off);
        }
        let _ = builder.is_test(true).try_init();
    }

    let worker_manager = WorkerManager::new();

    let worker = worker_manager
        .create_worker(worker_settings)
        .await
        .expect("Failed to create worker");

    let router = worker
        .create_router(RouterOptions::new(media_codecs()))
        .await
        .expect("Failed to create router");

    let transport = router
        .create_direct_transport(DirectTransportOptions::default())
        .await
        .expect("Failed to create direct transport");

    (worker, router, transport)
}

async fn create_audio_level_observer(router: &Router) -> AudioLevelObserver {
    let mut options = AudioLevelObserverOptions::default();
    options.interval = AUDIO_LEVEL_INTERVAL_MS;

    router
        .create_audio_level_observer(options)
        .await
        .expect("Failed to create AudioLevelObserver")
}

async fn produce_audio(transport: &DirectTransport, ssrc: u32) -> (ProducerId, DirectProducer) {
    let producer = transport
        .produce(audio_producer_options(ssrc))
        .await
        .expect("Failed to produce audio");
    let producer_id = producer.id();

    match producer {
        Producer::Direct(producer) => (producer_id, producer),
        _ => panic!("Direct transport must create direct producer"),
    }
}

async fn wait_for(what: &str, flag: &AtomicBool) {
    let started = Instant::now();

    while !flag.load(Ordering::SeqCst) {
        assert!(
            started.elapsed() < NOTIFICATION_DEADLINE,
            "{what} did not happen within {NOTIFICATION_DEADLINE:?}",
        );
        Timer::after(Duration::from_millis(10)).await;
    }
}

/// Worker thread that is parked never answers, so a response proves it is still running.
async fn assert_worker_is_responsive(router: &Router) {
    router
        .dump()
        .await
        .expect("Failed to dump router, worker is not responsive");
}

/// The application releases its only producer handle from another thread while `on_volumes` for
/// that producer is running, which leaves the handle given to the callback as the last one.
#[test]
fn last_producer_handle_released_while_volumes_are_reported() {
    run_scenario("released_while_volumes_are_reported", || {
        future::block_on(async move {
            let (_worker, router, transport) = init().await;
            let audio_level_observer = create_audio_level_observer(&router).await;

            let ssrc = 11111111;
            let (producer_id, producer) = produce_audio(&transport, ssrc).await;

            let volumes_reported = Arc::new(AtomicBool::new(false));
            let producer_released = Arc::new(AtomicBool::new(false));
            let release_timed_out = Arc::new(AtomicBool::new(false));

            audio_level_observer
                .on_volumes({
                    let volumes_reported = Arc::clone(&volumes_reported);
                    let producer_released = Arc::clone(&producer_released);
                    let release_timed_out = Arc::clone(&release_timed_out);

                    move |volumes| {
                        if volumes.is_empty() {
                            return;
                        }

                        volumes_reported.store(true, Ordering::SeqCst);

                        // Keep reported volumes alive until application handle is gone.
                        let started = Instant::now();
                        while !producer_released.load(Ordering::SeqCst)
                            && started.elapsed() < NOTIFICATION_DEADLINE
                        {
                            thread::sleep(Duration::from_millis(1));
                        }

                        // Panic would abort the process here, scenario thread asserts instead.
                        if !producer_released.load(Ordering::SeqCst) {
                            release_timed_out.store(true, Ordering::SeqCst);
                        }
                    }
                })
                .detach();

            audio_level_observer
                .add_producer(RtpObserverAddProducerOptions::new(producer_id))
                .await
                .expect("Failed to add producer to AudioLevelObserver");

            let feeder = thread::spawn({
                let volumes_reported = Arc::clone(&volumes_reported);
                let producer_released = Arc::clone(&producer_released);

                move || {
                    let mut feed = RtpFeed::new(ssrc);
                    let started = Instant::now();

                    while !volumes_reported.load(Ordering::SeqCst) {
                        assert!(
                            started.elapsed() < NOTIFICATION_DEADLINE,
                            "Volumes were not reported within {NOTIFICATION_DEADLINE:?}",
                        );
                        feed.send_next(&producer);
                        thread::sleep(RTP_PACKET_INTERVAL);
                    }

                    drop(producer);
                    producer_released.store(true, Ordering::SeqCst);
                }
            });

            feeder.join().expect("RTP feeder panicked");
            assert!(
                !release_timed_out.load(Ordering::SeqCst),
                "Producer was not released while its volumes were reported",
            );

            assert_worker_is_responsive(&router).await;
        });
    });
}

/// The application releases its only producer handle from inside `on_volumes` for that producer,
/// which leaves the handle given to the callback as the last one: the library drops it on the
/// worker thread right after the callback returns.
#[test]
fn producer_released_from_volumes_callback() {
    run_scenario("producer_released_from_volumes_callback", || {
        future::block_on(async move {
            let (_worker, router, transport) = init().await;
            let audio_level_observer = create_audio_level_observer(&router).await;

            let ssrc = 22222222;
            let (producer_id, producer) = produce_audio(&transport, ssrc).await;
            let producer = Arc::new(Mutex::new(Some(producer)));

            let producer_released = Arc::new(AtomicBool::new(false));

            audio_level_observer
                .on_volumes({
                    let producer = Arc::clone(&producer);
                    let producer_released = Arc::clone(&producer_released);

                    move |volumes| {
                        if volumes.is_empty() {
                            return;
                        }

                        drop(producer.lock().take());
                        producer_released.store(true, Ordering::SeqCst);
                    }
                })
                .detach();

            audio_level_observer
                .add_producer(RtpObserverAddProducerOptions::new(producer_id))
                .await
                .expect("Failed to add producer to AudioLevelObserver");

            let mut feed = RtpFeed::new(ssrc);
            let started = Instant::now();
            loop {
                assert!(
                    started.elapsed() < NOTIFICATION_DEADLINE,
                    "Volumes were not reported within {NOTIFICATION_DEADLINE:?}",
                );
                {
                    // Not held while sleeping: the callback takes this lock on the worker thread.
                    let producer = producer.lock();
                    let Some(producer) = producer.as_ref() else {
                        break;
                    };
                    feed.send_next(producer);
                }
                thread::sleep(RTP_PACKET_INTERVAL);
            }

            wait_for("Producer release", &producer_released).await;

            assert_worker_is_responsive(&router).await;
        });
    });
}

/// The application releases the only handle of an entity the notification is not about from
/// inside a notification callback.
#[test]
fn unrelated_producer_released_from_volumes_callback() {
    run_scenario("unrelated_producer_released_from_volumes_callback", || {
        future::block_on(async move {
            let (_worker, router, transport) = init().await;
            let audio_level_observer = create_audio_level_observer(&router).await;

            let ssrc = 33333333;
            let (producer_id, producer) = produce_audio(&transport, ssrc).await;
            let (_, unrelated_producer) = produce_audio(&transport, 44444444).await;
            let unrelated_producer = Arc::new(Mutex::new(Some(unrelated_producer)));

            let volumes_reported = Arc::new(AtomicBool::new(false));
            let unrelated_producer_released = Arc::new(AtomicBool::new(false));

            audio_level_observer
                .on_volumes({
                    let unrelated_producer = Arc::clone(&unrelated_producer);
                    let volumes_reported = Arc::clone(&volumes_reported);
                    let unrelated_producer_released = Arc::clone(&unrelated_producer_released);

                    move |volumes| {
                        if volumes.is_empty() {
                            return;
                        }

                        volumes_reported.store(true, Ordering::SeqCst);
                        drop(unrelated_producer.lock().take());
                        unrelated_producer_released.store(true, Ordering::SeqCst);
                    }
                })
                .detach();

            audio_level_observer
                .add_producer(RtpObserverAddProducerOptions::new(producer_id))
                .await
                .expect("Failed to add producer to AudioLevelObserver");

            let mut feed = RtpFeed::new(ssrc);
            let started = Instant::now();
            while !volumes_reported.load(Ordering::SeqCst) {
                assert!(
                    started.elapsed() < NOTIFICATION_DEADLINE,
                    "Volumes were not reported within {NOTIFICATION_DEADLINE:?}",
                );
                feed.send_next(&producer);
                thread::sleep(RTP_PACKET_INTERVAL);
            }

            wait_for("Unrelated producer release", &unrelated_producer_released).await;

            assert_worker_is_responsive(&router).await;
        });
    });
}

/// The application releases its only producer handle from inside `on_dominant_speaker` for that
/// producer, which leaves the handle given to the callback as the last one: the library drops it
/// on the worker thread right after the callback returns.
#[test]
fn producer_released_from_dominant_speaker_callback() {
    run_scenario("producer_released_from_dominant_speaker_callback", || {
        future::block_on(async move {
            let (_worker, router, transport) = init().await;

            let active_speaker_observer = router
                .create_active_speaker_observer(ActiveSpeakerObserverOptions::default())
                .await
                .expect("Failed to create ActiveSpeakerObserver");

            let ssrc = 55555555;
            let (producer_id, producer) = produce_audio(&transport, ssrc).await;
            let producer = Arc::new(Mutex::new(Some(producer)));

            let producer_released = Arc::new(AtomicBool::new(false));

            active_speaker_observer
                .on_dominant_speaker({
                    let producer = Arc::clone(&producer);
                    let producer_released = Arc::clone(&producer_released);

                    move |_dominant_speaker| {
                        drop(producer.lock().take());
                        producer_released.store(true, Ordering::SeqCst);
                    }
                })
                .detach();

            active_speaker_observer
                .add_producer(RtpObserverAddProducerOptions::new(producer_id))
                .await
                .expect("Failed to add producer to ActiveSpeakerObserver");

            let mut feed = RtpFeed::new(ssrc);
            let started = Instant::now();
            loop {
                assert!(
                    started.elapsed() < NOTIFICATION_DEADLINE,
                    "Dominant speaker was not reported within {NOTIFICATION_DEADLINE:?}",
                );
                {
                    // Not held while sleeping: the callback takes this lock on the worker thread.
                    let producer = producer.lock();
                    let Some(producer) = producer.as_ref() else {
                        break;
                    };
                    feed.send_next(producer);
                }
                thread::sleep(RTP_PACKET_INTERVAL);
            }

            wait_for("Producer release", &producer_released).await;

            assert_worker_is_responsive(&router).await;
        });
    });
}

/// The application releases every handle it has while `on_volumes` is running. What the running
/// notification owns (reported producers and the callback itself, which holds the router) is then
/// the last owner of the router, the worker and the worker manager, so all of them are released on
/// the worker thread, which must not end up waiting for its own exit.
#[test]
fn every_handle_released_while_volumes_are_reported() {
    run_scenario("every_handle_released_while_volumes_are_reported", || {
        let (worker_thread_exit_tx, worker_thread_exit_rx) = mpsc::channel();

        future::block_on(async move {
            let mut worker_settings = WorkerSettings::default();
            worker_settings.thread_initializer = Some(Arc::new(move || {
                let signal = ThreadExitSignal(worker_thread_exit_tx.clone());
                THREAD_EXIT_SIGNAL.with(|slot| slot.replace(Some(signal)));
            }));

            let (worker, router, transport) = init_with_settings(worker_settings).await;
            let audio_level_observer = create_audio_level_observer(&router).await;

            let ssrc = 66666666;
            let (producer_id, producer) = produce_audio(&transport, ssrc).await;

            let volumes_reported = Arc::new(AtomicBool::new(false));
            let everything_released = Arc::new(AtomicBool::new(false));

            audio_level_observer
                .on_volumes({
                    let volumes_reported = Arc::clone(&volumes_reported);
                    let everything_released = Arc::clone(&everything_released);

                    move |volumes| {
                        if volumes.is_empty() {
                            return;
                        }

                        volumes_reported.store(true, Ordering::SeqCst);

                        // Keep the notification running until application handles are gone.
                        let started = Instant::now();
                        while !everything_released.load(Ordering::SeqCst)
                            && started.elapsed() < NOTIFICATION_DEADLINE
                        {
                            thread::sleep(Duration::from_millis(1));
                        }
                    }
                })
                .detach();

            audio_level_observer
                .add_producer(RtpObserverAddProducerOptions::new(producer_id))
                .await
                .expect("Failed to add producer to AudioLevelObserver");

            let mut feed = RtpFeed::new(ssrc);
            let started = Instant::now();
            while !volumes_reported.load(Ordering::SeqCst) {
                assert!(
                    started.elapsed() < NOTIFICATION_DEADLINE,
                    "Volumes were not reported within {NOTIFICATION_DEADLINE:?}",
                );
                feed.send_next(&producer);
                thread::sleep(RTP_PACKET_INTERVAL);
            }

            drop(producer);
            drop(audio_level_observer);
            drop(transport);
            drop(router);
            drop(worker);
            everything_released.store(true, Ordering::SeqCst);
        });

        worker_thread_exit_rx
            .recv_timeout(NOTIFICATION_DEADLINE)
            .expect("Worker thread did not exit, it waits for itself in `WorkerManager` drop");
    });
}

/// Pause state of a data consumer can be read from its own pause/resume callbacks, both the ones
/// caused by the application and the ones caused by data producer notifications.
#[test]
fn data_consumer_pause_state_read_from_pause_callbacks() {
    #[derive(Debug, Eq, PartialEq)]
    enum Event {
        Pause,
        Resume,
        DataProducerPause,
        DataProducerResume,
    }

    #[derive(Debug, Eq, PartialEq)]
    struct Observed {
        event: Event,
        paused: bool,
        producer_paused: bool,
    }

    run_scenario(
        "data_consumer_pause_state_read_from_pause_callbacks",
        || {
            future::block_on(async move {
                let (_worker, router, transport) = init().await;

                let data_producer = transport
                    .produce_data(DataProducerOptions::new_direct())
                    .await
                    .expect("Failed to produce data");

                let data_consumer = transport
                    .consume_data(DataConsumerOptions::new_direct(data_producer.id(), None))
                    .await
                    .expect("Failed to consume data");

                // Callbacks caused by notifications run on the worker thread, where panic would
                // abort the process, so they only record what they see.
                let observed = Arc::new(Mutex::new(Vec::new()));
                let observe = {
                    let data_consumer = data_consumer.downgrade();
                    let observed = Arc::clone(&observed);

                    move |event| {
                        if let Some(data_consumer) = data_consumer.upgrade() {
                            observed.lock().push(Observed {
                                event,
                                paused: data_consumer.paused(),
                                producer_paused: data_consumer.producer_paused(),
                            });
                        }
                    }
                };

                data_consumer
                    .on_pause({
                        let observe = observe.clone();

                        move || observe(Event::Pause)
                    })
                    .detach();
                data_consumer
                    .on_resume({
                        let observe = observe.clone();

                        move || observe(Event::Resume)
                    })
                    .detach();
                data_consumer
                    .on_data_producer_pause({
                        let observe = observe.clone();

                        move || observe(Event::DataProducerPause)
                    })
                    .detach();
                data_consumer
                    .on_data_producer_resume({
                        let observe = observe.clone();

                        move || observe(Event::DataProducerResume)
                    })
                    .detach();

                let wait_for_events = |count: usize| {
                    let observed = Arc::clone(&observed);

                    async move {
                        let started = Instant::now();

                        while observed.lock().len() < count {
                            assert!(
                                started.elapsed() < NOTIFICATION_DEADLINE,
                                "Expected {count} events within {NOTIFICATION_DEADLINE:?}",
                            );
                            Timer::after(Duration::from_millis(10)).await;
                        }
                    }
                };

                data_consumer
                    .pause()
                    .await
                    .expect("Failed to pause data consumer");
                data_consumer
                    .resume()
                    .await
                    .expect("Failed to resume data consumer");
                wait_for_events(2).await;

                data_producer
                    .pause()
                    .await
                    .expect("Failed to pause data producer");
                wait_for_events(4).await;

                data_producer
                    .resume()
                    .await
                    .expect("Failed to resume data producer");
                wait_for_events(6).await;

                let expected = |event, paused, producer_paused| Observed {
                    event,
                    paused,
                    producer_paused,
                };
                assert_eq!(
                    *observed.lock(),
                    [
                        expected(Event::Pause, true, false),
                        expected(Event::Resume, false, false),
                        expected(Event::DataProducerPause, false, true),
                        expected(Event::Pause, false, true),
                        expected(Event::DataProducerResume, false, false),
                        expected(Event::Resume, false, false),
                    ],
                );

                assert_worker_is_responsive(&router).await;
            });
        },
    );
}

/// The application releases the only handle of a data consumer from inside that data consumer's
/// own `on_data_producer_pause`, which removes the subscription whose callback is running.
#[test]
fn data_consumer_released_from_its_own_notification_callback() {
    run_scenario(
        "data_consumer_released_from_its_own_notification_callback",
        || {
            future::block_on(async move {
                let (_worker, router, transport) = init().await;

                let data_producer = transport
                    .produce_data(DataProducerOptions::new_direct())
                    .await
                    .expect("Failed to produce data");

                let data_consumer = transport
                    .consume_data(DataConsumerOptions::new_direct(data_producer.id(), None))
                    .await
                    .expect("Failed to consume data");

                let data_consumer_closed = Arc::new(AtomicBool::new(false));
                let closed_when_released = Arc::new(AtomicBool::new(false));
                let data_consumer_released = Arc::new(AtomicBool::new(false));

                data_consumer
                    .on_close({
                        let data_consumer_closed = Arc::clone(&data_consumer_closed);

                        move || data_consumer_closed.store(true, Ordering::SeqCst)
                    })
                    .detach();

                let data_consumer = Arc::new(Mutex::new(Some(data_consumer)));

                data_consumer
                    .lock()
                    .as_ref()
                    .expect("Data consumer was just stored")
                    .on_data_producer_pause({
                        let data_consumer = Arc::clone(&data_consumer);
                        let data_consumer_closed = Arc::clone(&data_consumer_closed);
                        let closed_when_released = Arc::clone(&closed_when_released);
                        let data_consumer_released = Arc::clone(&data_consumer_released);

                        move || {
                            let last_handle = data_consumer.lock().take();
                            drop(last_handle);

                            // Panic would abort the process here, scenario thread asserts instead.
                            closed_when_released.store(
                                data_consumer_closed.load(Ordering::SeqCst),
                                Ordering::SeqCst,
                            );
                            data_consumer_released.store(true, Ordering::SeqCst);
                        }
                    })
                    .detach();

                data_producer
                    .pause()
                    .await
                    .expect("Failed to pause data producer");

                wait_for("Data consumer release", &data_consumer_released).await;
                assert!(
                    closed_when_released.load(Ordering::SeqCst),
                    "Handle released from the callback was not the last one",
                );

                assert_worker_is_responsive(&router).await;
            });
        },
    );
}
