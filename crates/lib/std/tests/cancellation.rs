use std::{error::Error, future::Future, pin::pin, task::Context, task::Poll, task::Waker};

use rg_std::{Cancelable, CancellationToken, Cancelled, OperationError};

struct NameCollector {
    cancellation: CancellationToken,
    names: Vec<String>,
}

impl Cancelable for NameCollector {
    fn check_cancelled(&self, checkpoint: &'static str) -> Result<(), Cancelled> {
        self.cancellation.check_cancelled(checkpoint)
    }
}

impl NameCollector {
    #[rg_std::cancelable]
    fn add(&mut self, name: &str) -> Result<(), Cancelled> {
        self.names.push(name.to_owned());
        Ok(())
    }

    #[rg_std::cancelable("collect names")]
    fn collect(&mut self, names: impl IntoIterator<Item = String>) -> Result<(), Cancelled> {
        for name in names {
            rg_std::check_cancel!(self, "next name");
            self.names.push(name);
        }
        Ok(())
    }

    #[rg_std::cancelable("select name", token = cancellation)]
    fn select<'a, T>(
        cancellation: &CancellationToken,
        names: &'a [T],
    ) -> Result<Option<&'a T>, Cancelled>
    where
        T: AsRef<str>,
    {
        Ok(names.iter().find(|name| !name.as_ref().is_empty()))
    }
}

#[test]
fn entry_checks_stop_work_and_keep_the_method_label() {
    let mut collector = NameCollector {
        cancellation: CancellationToken::new(),
        names: Vec::new(),
    };
    collector.add("Widget").expect("collection is active");
    collector.cancellation.cancel();

    let cancelled = collector
        .add("Gadget")
        .expect_err("collection was cancelled");
    assert_eq!(collector.names, ["Widget"]);
    assert_eq!(cancelled.checkpoint(), concat!(module_path!(), "::add"));
}

#[test]
fn loop_checks_stop_between_items() {
    let mut collector = NameCollector {
        cancellation: CancellationToken::new(),
        names: Vec::new(),
    };
    let cancellation = collector.cancellation.clone();
    let names = ["Widget", "Gadget"]
        .into_iter()
        .enumerate()
        .map(|(index, name)| {
            if index == 1 {
                cancellation.cancel();
            }
            name.to_owned()
        });

    let cancelled = collector
        .collect(names)
        .expect_err("second item cancels collection");
    assert_eq!(collector.names, ["Widget"]);
    assert_eq!(cancelled.checkpoint(), "next name");

    let cancelled = collector
        .collect(Vec::new())
        .expect_err("entry is cancelled");
    assert_eq!(cancelled.checkpoint(), "collect names");
}

#[test]
fn explicit_tokens_preserve_generic_signatures_and_borrowed_results() {
    let cancellation = CancellationToken::new();
    let names = [String::new(), "Widget".to_owned()];
    let selected = NameCollector::select(&cancellation, &names).expect("selection is active");
    assert!(std::ptr::eq(selected.expect("nonempty name"), &names[1]));

    cancellation.cancel();
    let cancelled =
        NameCollector::select(&cancellation, &names).expect_err("selection is cancelled");
    assert_eq!(cancelled.checkpoint(), "select name");
}

#[rg_std::cancelable(token = cancellation)]
async fn select_async(cancellation: &CancellationToken) -> Result<&'static str, Cancelled> {
    Ok("Widget")
}

#[test]
fn async_entry_checks_run_when_execution_starts() {
    let cancellation = CancellationToken::new();
    let mut context = Context::from_waker(Waker::noop());
    assert_eq!(
        pin!(select_async(&cancellation)).poll(&mut context),
        Poll::Ready(Ok("Widget")),
    );

    let future = select_async(&cancellation);
    cancellation.cancel();
    let Poll::Ready(Err(cancelled)) = pin!(future).poll(&mut context) else {
        panic!("cancelled before execution");
    };
    assert_eq!(
        cancelled.checkpoint(),
        concat!(module_path!(), "::select_async")
    );
}

#[test]
fn explicit_checks_borrow_the_source_once_and_identify_the_call_site() {
    let mut cancellation = CancellationToken::new();
    cancellation.cancel();
    let source: &mut dyn Cancelable = &mut cancellation;
    let mut evaluations = 0;
    let result = (|| -> Result<(), Cancelled> {
        rg_std::check_cancel!({
            evaluations += 1;
            &source
        });
        Ok(())
    })();

    assert_eq!(evaluations, 1);
    let cancelled = result.expect_err("source was cancelled");
    assert!(
        cancelled
            .checkpoint()
            .starts_with(concat!(module_path!(), ":"))
    );
}

#[test]
fn cancellation_and_source_errors_keep_their_identity() {
    let cancellation = CancellationToken::new();
    let source_error = (|| -> Result<(), OperationError<std::io::Error>> {
        rg_std::check_cancel!(cancellation);
        // A source failure still belongs to the source even if cancellation arrives during it.
        cancellation.cancel();
        Err(OperationError::Source(std::io::Error::other(
            "read declaration shard",
        )))
    })()
    .expect_err("source failed");
    assert!(matches!(source_error, OperationError::Source(_)));

    let cancelled = (|| -> Result<(), OperationError<std::convert::Infallible>> {
        rg_std::check_cancel!(cancellation, "read declarations");
        Ok(())
    })()
    .expect_err("operation was cancelled");
    assert!(matches!(cancelled, OperationError::Cancelled(_)));
    let cause = cancelled.source().expect("typed cancellation cause");
    assert_eq!(
        cause
            .downcast_ref::<Cancelled>()
            .expect("cancellation is preserved")
            .checkpoint(),
        "read declarations"
    );

    let erased = (|| -> Result<(), Box<dyn Error>> {
        rg_std::check_cancel!(cancellation, "erased error");
        Ok(())
    })()
    .expect_err("operation was cancelled");
    assert_eq!(
        erased
            .downcast_ref::<Cancelled>()
            .expect("cancellation is preserved")
            .checkpoint(),
        "erased error"
    );
}
