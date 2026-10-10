use std::fmt::Write;
use std::time::Duration;

use crate::types::{Boundary, BoundaryEvent};

pub fn to_srt(events: &[BoundaryEvent]) -> String {
    let mut output = String::with_capacity(events.len() * 64);
    for (index, event) in events.iter().enumerate() {
        let _ = writeln!(output, "{}", index + 1);
        write_srt_timestamp(&mut output, ticks_to_duration(event.offset_ticks));
        output.push_str(" --> ");
        write_srt_timestamp(
            &mut output,
            ticks_to_duration(event.offset_ticks + event.duration_ticks),
        );
        output.push('\n');
        let _ = writeln!(output, "{}", event.text);
        let _ = writeln!(output);
    }
    output
}

pub fn filter_boundaries(events: &[BoundaryEvent], boundary: Boundary) -> Vec<BoundaryEvent> {
    events
        .iter()
        .filter(|event| event.kind == boundary)
        .cloned()
        .collect()
}

fn ticks_to_duration(ticks: u64) -> Duration {
    Duration::from_micros(ticks / 10)
}

fn write_srt_timestamp(output: &mut String, duration: Duration) {
    let total_millis = duration.as_millis();
    let hours = total_millis / 3_600_000;
    let minutes = (total_millis % 3_600_000) / 60_000;
    let seconds = (total_millis % 60_000) / 1_000;
    let millis = total_millis % 1_000;
    let _ = write!(output, "{hours:02}:{minutes:02}:{seconds:02},{millis:03}");
}
