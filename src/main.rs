use field_service_failure_loop::{
    process_work_order, DispatchStatus, InfraiClient, LoopDecision, WorkOrderEvidence,
};
use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 5 {
        return Err("usage: dispatch_guard WORK_ORDER_ID STATUS PHOTO_COUNT FOLLOW_UP".into());
    }
    let input = WorkOrderEvidence {
        work_order_id: args[1].clone(),
        dispatch_status: DispatchStatus::parse(&args[2])?,
        photo_count: args[3].parse()?,
        technician_follow_up: args[4].parse()?,
    };
    let client = InfraiClient::from_env()?;
    let decision = block_on(process_work_order(&client, input))?;
    match decision {
        LoopDecision::Continue => println!("continue: evidence gate passed"),
        LoopDecision::HoldForFollowUp(_) => {
            println!("hold-for-follow-up: failure captured")
        }
    }
    Ok(())
}

struct ThreadWake;
impl Wake for ThreadWake {
    fn wake(self: Arc<Self>) {}
}

fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(ThreadWake));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
        std::thread::yield_now();
    }
}
