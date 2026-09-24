//! Compile the public helper against all SDK contexts, including borrowed,
//! non-serializable, non-Clone pending values and Send handler futures.

use std::{
    future::{Future, ready},
    ops::ControlFlow,
    time::Duration,
};

use restate_ext::poll::Poller;
use restate_sdk::prelude::*;

struct Snapshot<'a> {
    _value: &'a str,
}

fn assert_send(_: impl Future + Send) {}

#[test]
fn all_contexts_support_borrowed_observations() {
    macro_rules! check {
        ($($context:ident),+ $(,)?) => {$(
            let _check = |ctx: $context<'_>| {
                let value = String::from("pending");

                let poller = Poller::builder(Duration::from_secs(1))
                    .timeout(Duration::ZERO)
                    .build()
                    .unwrap();

                assert_send(poller.poll(
                    &ctx,
                    || ready(Ok(ControlFlow::<(), _>::Continue(Snapshot { _value: &value }))),
                ));
            };
        )+};
    }

    check!(
        Context,
        ObjectContext,
        SharedObjectContext,
        WorkflowContext,
        SharedWorkflowContext
    );
}
