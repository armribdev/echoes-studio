//! What a VST3 controller hears about its track, through a hand-made plugin: the
//! real ones are covered by `ffi_lifecycle_tests`.

use std::time::Duration;

use crate::plugin_formats::clap::ffi::HostTrackInfo;
use crate::plugin_formats::vst3::fake_plugin::{fake_vst3_instance, FakeListener, Received};
use crate::plugin_system::instance::{take_undelivered_track_info, UNDELIVERED_FLAG_TEST_LOCK};

fn lead() -> HostTrackInfo {
    HostTrackInfo::new("Lead", Some([0x10, 0x20, 0x30]))
}

fn lead_received() -> Received {
    Received {
        name: Some("Lead".into()),
        name_length: Some(4),
        color: Some(0xFF10_2030),
    }
}

#[test]
fn a_controller_hears_the_name_and_color_of_its_track() {
    let mut listener = FakeListener::new(true);
    let instance = fake_vst3_instance(&mut listener);

    instance.set_track_info(&lead());

    assert_eq!(*listener.received.borrow(), [lead_received()]);
    assert_eq!(listener.references.get(), 1, "no reference is left behind");
}

#[test]
fn the_same_track_is_not_announced_twice() {
    let mut listener = FakeListener::new(true);
    let instance = fake_vst3_instance(&mut listener);

    instance.set_track_info(&lead());
    instance.set_track_info(&lead());
    instance.notify_track_info_changed();

    assert_eq!(listener.received.borrow().len(), 1);
}

#[test]
fn a_rename_and_a_removed_color_are_announced() {
    let mut listener = FakeListener::new(true);
    let instance = fake_vst3_instance(&mut listener);
    instance.set_track_info(&lead());

    instance.set_track_info(&HostTrackInfo::new("Pad", Some([0x10, 0x20, 0x30])));
    instance.set_track_info(&HostTrackInfo::new("Pad", None));

    let received = listener.received.borrow();
    assert_eq!(received.len(), 3);
    assert_eq!(received[1].name.as_deref(), Some("Pad"));
    assert_eq!(received[1].color, Some(0xFF10_2030));
    assert_eq!(
        received[2],
        Received {
            name: Some("Pad".into()),
            name_length: Some(3),
            color: None,
        }
    );
}

#[test]
fn a_track_with_neither_name_nor_color_is_not_announced_to_a_new_plugin() {
    let mut listener = FakeListener::new(true);
    let instance = fake_vst3_instance(&mut listener);

    instance.set_track_info(&HostTrackInfo::new("", None));

    assert!(listener.received.borrow().is_empty());
}

#[test]
fn a_track_that_loses_everything_is_announced_as_empty() {
    let mut listener = FakeListener::new(true);
    let instance = fake_vst3_instance(&mut listener);
    instance.set_track_info(&lead());

    instance.set_track_info(&HostTrackInfo::new("", None));

    assert_eq!(
        listener.received.borrow()[1],
        Received {
            name: None,
            name_length: None,
            color: None,
        }
    );
}

#[test]
fn a_controller_without_the_interface_is_left_alone() {
    let mut listener = FakeListener::new(false);
    let instance = fake_vst3_instance(&mut listener);

    instance.set_track_info(&lead());
    instance.notify_track_info_changed();

    assert!(listener.received.borrow().is_empty());
    assert_eq!(listener.references.get(), 1);
}

/// The undelivered flag is process-wide: the tests that raise or read it take the lock.
#[test]
fn a_notification_missed_while_the_plugin_is_busy_is_retried() {
    let _flag = UNDELIVERED_FLAG_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut listener = FakeListener::new(true);
    let instance = fake_vst3_instance(&mut listener);
    take_undelivered_track_info();

    {
        let _block_in_flight = instance
            .offrt_try_acquire(Duration::from_millis(1))
            .unwrap();
        instance.set_track_info(&lead());
        assert!(listener.received.borrow().is_empty(), "busy");
    }
    assert!(take_undelivered_track_info(), "the app is asked to retry");

    instance.notify_track_info_changed();
    assert_eq!(*listener.received.borrow(), [lead_received()]);
    instance.notify_track_info_changed();
    assert_eq!(listener.received.borrow().len(), 1, "delivered once");
    assert!(!take_undelivered_track_info());
}

#[test]
fn the_latest_track_is_the_one_delivered_after_a_busy_period() {
    let _flag = UNDELIVERED_FLAG_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut listener = FakeListener::new(true);
    let instance = fake_vst3_instance(&mut listener);

    {
        let _block_in_flight = instance
            .offrt_try_acquire(Duration::from_millis(1))
            .unwrap();
        instance.set_track_info(&lead());
        instance.set_track_info(&HostTrackInfo::new("Pad", None));
    }
    take_undelivered_track_info();
    instance.notify_track_info_changed();

    let received = listener.received.borrow();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].name.as_deref(), Some("Pad"));
    assert_eq!(received[0].color, None);
}
