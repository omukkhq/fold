use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use fold_core::events::types;
use fold_core::{
    Event, EventActor, EventFilter, EventType, LiveEvent, NewEvent, TaskId, Timestamp,
};
use fold_daemon::services::events::{BusMessage, EventBuffer, EventBus, EventStream};
use fold_store::testing::{TestProject, eventually};
use fold_store::{Store, StoreError, events};
use serde_json::json;
use tokio::sync::broadcast::error::{RecvError, TryRecvError};

#[cfg(test)]
fn new_event(event_type: EventType, task: Option<u32>) -> NewEvent {
    let event = NewEvent::new(event_type, EventActor::parse("frontend").unwrap()).unwrap();
    match task {
        Some(number) => event.task(TaskId::from_number(number).unwrap()),
        None => event,
    }
}

/// One service-shaped write: append into a buffer inside the transaction, publish after it commits.
#[cfg(test)]
async fn commit(store: &Store, bus: &EventBus, now: Timestamp, batch: &[NewEvent]) {
    let mut buffer = EventBuffer::new();
    store
        .write(async |tx| {
            for event in batch {
                buffer.append(tx, event, now).await?;
            }
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    bus.publish(buffer);
}

#[cfg(test)]
async fn next_event(stream: &mut EventStream) -> Arc<Event> {
    tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("the stream stalled")
        .expect("the stream ended")
        .unwrap()
}

#[cfg(test)]
fn durable(message: BusMessage) -> Arc<Event> {
    match message {
        BusMessage::Durable(event) => event,
        BusMessage::Live(_) => unreachable!("expected a durable event"),
    }
}

#[tokio::test]
async fn a_committed_write_publishes_in_seq_order_only_after_the_commit() {
    let project = TestProject::new().await;
    let bus = EventBus::new();
    let mut receiver = bus.subscribe();
    let mut buffer = EventBuffer::new();
    assert!(buffer.is_empty());
    project
        .store()
        .write(async |tx| {
            for task in 1..=3 {
                let event = new_event(types::TASK_CREATED, Some(task));
                buffer.append(tx, &event, project.now()).await?;
            }
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    assert_eq!(buffer.len(), 3);
    assert!(
        matches!(receiver.try_recv(), Err(TryRecvError::Empty)),
        "nothing is published before publish is called"
    );
    bus.publish(buffer);
    let seqs: Vec<u64> = (0..3)
        .map(|_| durable(receiver.try_recv().unwrap()).seq)
        .collect();
    assert_eq!(seqs, vec![1, 2, 3]);
    assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
}

#[tokio::test]
async fn a_rolled_back_write_leaves_no_row_and_publishes_nothing() {
    let project = TestProject::new().await;
    let bus = EventBus::new();
    let mut receiver = bus.subscribe();
    let mut buffer = EventBuffer::new();
    let outcome = project
        .store()
        .write(async |tx| {
            buffer
                .append(tx, &new_event(types::TASK_CREATED, Some(1)), project.now())
                .await?;
            Err::<(), _>(StoreError::Invalid("service refused"))
        })
        .await;
    assert!(outcome.is_err());
    drop(buffer);
    assert_eq!(events::head_seq(project.store().read()).await.unwrap(), 0);
    let stored = events::read_after(project.store().read(), 0, &EventFilter::default(), 10)
        .await
        .unwrap();
    assert!(stored.is_empty());
    assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
}

#[tokio::test]
async fn publish_orders_a_buffer_by_seq_and_live_events_have_no_seq() {
    let project = TestProject::new().await;
    let bus = EventBus::default();
    let mut receiver = bus.subscribe();
    let live = LiveEvent::new(types::WORKSPACE_CHANGED, None, json!({"files": 1})).unwrap();
    bus.publish_live(live.clone());
    match receiver.try_recv().unwrap() {
        BusMessage::Live(received) => assert_eq!(*received, live),
        BusMessage::Durable(_) => unreachable!("expected a live event"),
    }
    commit(
        project.store(),
        &bus,
        project.now(),
        &[new_event(types::TASK_CREATED, None)],
    )
    .await;
    assert_eq!(durable(receiver.try_recv().unwrap()).seq, 1);
}

#[tokio::test]
async fn a_lagging_subscriber_recovers_every_event_through_stream_after() {
    let project = TestProject::new().await;
    let bus = EventBus::with_capacity(4);
    let mut raw = bus.subscribe();
    let mut stream = bus.stream_after(project.store(), 0, EventFilter::default());
    for task in 1..=50 {
        commit(
            project.store(),
            &bus,
            project.now(),
            &[new_event(types::TASK_CREATED, Some(task))],
        )
        .await;
    }
    assert!(matches!(raw.recv().await, Err(RecvError::Lagged(_))));
    let mut seen = Vec::new();
    for _ in 0..50 {
        seen.push(next_event(&mut stream).await.seq);
    }
    assert_eq!(seen, (1..=50).collect::<Vec<u64>>());

    commit(
        project.store(),
        &bus,
        project.now(),
        &[new_event(types::TASK_UPDATED, Some(1))],
    )
    .await;
    assert_eq!(next_event(&mut stream).await.seq, 51);
}

#[tokio::test]
async fn stream_after_reads_the_backlog_then_follows_live_events() {
    let project = TestProject::new().await;
    let bus = EventBus::new();
    for task in 1..=4 {
        commit(
            project.store(),
            &bus,
            project.now(),
            &[new_event(types::TASK_CREATED, Some(task))],
        )
        .await;
    }
    let mut stream = bus.stream_after(project.store(), 2, EventFilter::default());
    assert_eq!(next_event(&mut stream).await.seq, 3);
    assert_eq!(next_event(&mut stream).await.seq, 4);
    bus.publish_live(LiveEvent::new(types::WORKSPACE_CHANGED, None, json!({})).unwrap());
    commit(
        project.store(),
        &bus,
        project.now(),
        &[new_event(types::TASK_UPDATED, Some(1))],
    )
    .await;
    let live = next_event(&mut stream).await;
    assert_eq!((live.seq, live.event_type), (5, types::TASK_UPDATED));
}

#[tokio::test]
async fn stream_after_applies_its_filter_to_backlog_and_live_events() {
    let project = TestProject::new().await;
    let bus = EventBus::new();
    commit(
        project.store(),
        &bus,
        project.now(),
        &[
            new_event(types::TASK_CREATED, Some(1)),
            new_event(types::TASK_CREATED, Some(2)),
        ],
    )
    .await;
    let filter = EventFilter {
        task: TaskId::from_number(2),
        ..EventFilter::default()
    };
    let mut stream = bus.stream_after(project.store(), 0, filter);
    assert_eq!(next_event(&mut stream).await.seq, 2);
    commit(
        project.store(),
        &bus,
        project.now(),
        &[
            new_event(types::TASK_UPDATED, Some(1)),
            new_event(types::TASK_UPDATED, Some(3)),
            new_event(types::TASK_UPDATED, Some(2)),
            new_event(types::TASK_ARCHIVED, Some(2)),
        ],
    )
    .await;
    assert_eq!(next_event(&mut stream).await.seq, 5);
    assert_eq!(next_event(&mut stream).await.seq, 6);
}

#[tokio::test]
async fn stream_after_pages_through_a_backlog_larger_than_one_page() {
    let project = TestProject::new().await;
    let bus = EventBus::new();
    let batch: Vec<NewEvent> = (0..1500)
        .map(|_| new_event(types::TASK_UPDATED, None))
        .collect();
    commit(project.store(), &bus, project.now(), &batch).await;
    let mut stream = bus.stream_after(project.store(), 0, EventFilter::default());
    let mut last = 0;
    for _ in 0..1500 {
        let seq = next_event(&mut stream).await.seq;
        assert_eq!(seq, last + 1);
        last = seq;
    }
}

#[tokio::test]
async fn stream_after_yields_each_seq_once_under_concurrent_appends() {
    let project = TestProject::new().await;
    let bus = EventBus::with_capacity(8);
    let mut stream = bus.stream_after(project.store(), 0, EventFilter::default());
    let now = project.now();
    let mut writers = Vec::new();
    for writer in 0..8u32 {
        let store = project.store().clone();
        let bus = bus.clone();
        writers.push(tokio::spawn(async move {
            for round in 0..25u32 {
                let event = new_event(types::TASK_UPDATED, Some(writer * 100 + round + 1));
                commit(&store, &bus, now, &[event]).await;
            }
        }));
    }
    let mut seen = Vec::new();
    while seen.len() < 200 {
        seen.push(next_event(&mut stream).await.seq);
    }
    for writer in writers {
        writer.await.unwrap();
    }
    assert_eq!(seen, (1..=200).collect::<Vec<u64>>());
    let unique: BTreeSet<u64> = seen.iter().copied().collect();
    assert_eq!(unique.len(), 200);
    let extra = tokio::time::timeout(Duration::from_millis(50), stream.next()).await;
    assert!(extra.is_err(), "no further event may arrive");
}

#[tokio::test]
async fn the_stream_ends_when_the_bus_is_dropped_and_eventually_waits_for_it() {
    let project = TestProject::new().await;
    let bus = EventBus::new();
    let mut stream = bus.stream_after(project.store(), 0, EventFilter::default());
    let handle = tokio::spawn(async move { stream.next().await.is_none() });
    drop(bus);
    eventually(Duration::from_secs(5), async || {
        if handle.is_finished() {
            Ok(())
        } else {
            Err("stream still open")
        }
    })
    .await;
    assert!(handle.await.unwrap());
}

#[tokio::test]
async fn a_failing_table_read_is_reported_by_the_stream() {
    let project = TestProject::new().await;
    let bus = EventBus::new();
    project
        .store()
        .write(async |tx| {
            sqlx::query(
                "INSERT INTO events (type, actor, data, created_at) VALUES ('not.declared', 'fold', '{}', 0)",
            )
            .execute(tx.conn())
            .await?;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    let mut stream = bus.stream_after(project.store(), 0, EventFilter::default());
    let outcome = stream.next().await.unwrap();
    assert!(matches!(outcome, Err(StoreError::Corrupt("events.type"))));
}
