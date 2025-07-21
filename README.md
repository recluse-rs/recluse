`recluse` is both a crate and a guide to help you write web crawlers in Rust.
It was originally inspired by [Scrapy](https://www.scrapy.org/), a legendary Python framework for writing web scrapers
(which now calls itself a "data extraction framework") but it has deviated in its achitecture to be more Rust-idiomatic:
- Every parsing function in Scrapy `yield`s a host of different objects (items, URLs) into a single work queue.
  `recluse` still encourages you to use the concept of generating items if you've found something interesting,
  and URLs (or web request objects) if you want to signal a link that needs to be traversed.
  But we have to work with a strongly-typed language, so instead of forcing users to maintain "giga-`enum`s"
  that accomodate every "thing" that moves around, in `recluse` every type of "thing" has its own typed queue, and is
  statically bound to its consumer (no dynamic dispatch). Thanks to `tower`, it gives you more granular control per item type
  over things like throttles, retries, etc.
- There is no "master object" spider, no "context" provided to a parsing function.
  It is recommended to use `tower` layers to add stateful behaviour, if you need it.
  Parse pages with `scrapper`. Compose your workers into a working program with `tokio`.
  As you go through examples, you'll see that not a lot of core code comes from `recluse`.
  It's mostly boilerplate and common crawler tasks like downloading pages.
  Your program structure and control flow remains familiar and understandable.

You might also notice that the architecture that `recluse` proposes is essentially an
[actor model](https://en.wikipedia.org/wiki/Actor_model) but with fewer steps.
This was not intended, but did not come as a surprise, either.
The actor model is a great solution for many problems in software engineering, especially in
long-running, concurrent programs, by decoupling components narrowing the interface between them
to a simple message. This simplifies state management and increases predictability of responses.
That being said, `recluse` is *not* a full-blown actor model. Actors cannot have a local state,
there are no supervisors (although you could make those things with `tower` layers). Actor references
are static, with no addressing (your handler either has a channel sender or not) and there is no
overarching "actor system" (unless you count your async runtime as one).

# Dependencies

`recluse` tries its best not to force you into using specific crates or frameworks.
That being said, some choices have been made to make development more ergonomic:
- Most notably, `recluse` depends on [`tower`](https://docs.rs/tower/latest/tower/) for composing your workers.
  You will write your own [`tower::Service`] that processes a page and add [`tower::Layer`]s such as concurrency and rate limiting,
  retries, timeouts, etc. `recluse` even provides layers on its own, for crawler-specific code.
- We use the [`log`](https://docs.rs/log/latest/log/) facade, but don't depend on any specific logger.
  (Examples use [`colog`](https://docs.rs/colog/latest/colog/), but that's not a library dependency.)
- The fundamental building block of `recluse` makes heavy use of [`tokio`](https://docs.rs/tokio/latest/tokio/)'s `mpsc::channel`.
  That being said, `tokio` as a runtime is not a hard dependency and the channels are runtime-agnostic.
  You should be able to use this crate with any async runtime.
- [`reqwest`](https://docs.rs/reqwest/latest/reqwest/) is the most widely used HTTP client crate.
  Through it, `recluse` provides a convenient wrapper that downloads pages for you (as raw text, or JSON-deserialized
  with [`serde`](https://docs.rs/serde/latest/serde/)), so you can focus on processing.
  It's gated by an (optional) feature, `reqwest`, so you're opted out of including it by default.

# Quickstart

I recommend going through the examples included in this repository to understand how crawlers are built with `recluse`.

We'll go here through the first one, which simply extracts quotes from [Quotes to Scrape](https://quotes.toscrape.com/),
a popular website for developing and testing scrapers and crawlers. It will be a simple website-scoped spider
(that is, it doesn't wander off to different domains) that must find a link to the next page every time and
traverse them all.

We start by creating a "work pipe", which is `recluse`'s fundamental construct. It is wrapper around a [`tokio::sync::mpsc::channel`]
with an atomic counter that keeps track of how many items are in it (in fact, it tracks *all* work pipes in your program
with a single counter):

```rust
let (page_pipe, page_worker) = WorkPipeBuilder::new().build();
```

The pipe part is, essentially, the `Sender` part of the channel and you queue into it requests to download pages
(in this case, it's `reqwest::Request` objects, but you could use anything, e.g. your abstraction over it).
The other end of the channel is unavailable to you because it's wrapped by the worker - a simple-ish loop
that consumes work items by putting them through your processing function until there is no more work.

We then create the core function, which will be processing the pages as they are coming, one at a time.
This is the "meat" of our program, everything else is really either boilerplate or plumbing
(which `recluse` tries to minimize through utilities it provides):

```rust
let page_processor = {
    let page_pipe = page_pipe.clone();
    
    move |page_body: String| {
        let page_pipe = page_pipe.clone();
        
        async move {
            let (quotes, next_page_request) = parse_quotes_page(page_body)
                .context("Could not parse quotes page")?;

            debug!("{:?}", quotes);

            if let Some(next_page_request) = next_page_request {
                page_pipe.submit_work(next_page_request).await
                    .context("Queue next page request")?;
            }
            
            anyhow::Ok(())
        }
    }
};
```

Everything in `recluse` happens asynchronously (usually powered by the `tokio` runtime), therefore we ultimately produce
an async closure that takes a `String` and returns `()`. Because a lot of things can happen in a processor (you find items of interest,
links to further pages), we avoid forcing a specific return signature because unlike with Scrapy, a) we work in a strongly typed language,
and b) don't have generators/coroutines like in Python (not yet, anyway, Rust is coooking).

Instead, outgoing communication must be done via pipes. If your processor needs to send an item, or link, for processing,
just clone the pipe, move it into the closure, and submit work into it. Interestingly, in this case, our worker is simulatenously
the producer and consumer of a single channel. That could cause problems if it were a bare, say, `tokio::sync::mpsc::channel`,
but pipes contain additional logic that should resolve them. You can read more musings about this later,
under ["Regarding work counters"](#regarding-work-counters).

What `parse_quotes_page` does is not too important, but it's essentially what you'd write to parse the HTML and look for things of interest.
In this example, it returns the list of quotes it found, and (an `Option` of) a link to the next page.

We skip actually saving the quotes anywhere (you can see actual database interaction in later examples),
but a call to some `database_pipe.submit_work(quote).await` would happen here in an actual spider.

At the end we queue up a link to the next page. By the time this closure finishes, it will be available for processing.

Note that the examples use `anyhow` for error handling, however the crate itself does not, and returns custom error objects.
That is to say, using `recluse` does not pull `anyhow` as a dependency.

Next part uses `tower`'s service composition pattern to actually produce a `tower::Service`:

```rust
    let quotes_page_parser_service = tower::ServiceBuilder::new()
        .map_request(string_to_get_reqwest)
        .filter(print_errors)
        .rate_limit(1, Duration::from_secs(1))
        .layer(BodyDownloaderLayer)
        .service_fn(page_processor);
```

We start by attaching a mapper ([`recluse::string_to_get_reqwest`]) that takes [`String`]s and converts them to GET [`reqwest::Request`]s
(requires the `reqwest` feature of this crate).

Parsing a URL can fail, so we pass the result into [`recluse::print_errors`], which is an identity function with a side effect of
printing any errors coming through. `filter()` itself will remove any errors from the stream.

We then apply a simple 1/second throttle, but, if you're familiar with `tower`, you'll know that many more options exist,
like retries and timeouts, that would be appropriate in a crawler.

Next up is [`recluse::BodyDownloaderLayer`], which handles the HTTP Client for you (requires the `reqwest` feature of this crate),
such that you only write a function of `String`, not `reqwest::Request`.

And in case you're wondering, yes, there is a `recluse::JsonDownloaderLayer` which attempts to deserialize the HTTP response
into a strongly typed object of your choosing. Useful if you're traversing an API, not a website.

We then kick the worker into its own thread, here with `tokio`:

```rust
let worker = tokio::spawn(async move {
    page_worker.work(quotes_page_parser_service).await
});
```

And give the spider its initial page by simply queuing it into the same pipe:

```rust
page_pipe.submit_work("https://quotes.toscrape.com/".to_string()).await
    .context("Sent initial request")?;
```

You could kickstart as much work this way as you need.

Finally, we simply wait for the worker to complete:

```rust
tokio::join!(worker).0??;
```

As mentioned previously, the worker closures return unit (`()`), so we simply ignore the result here.

Analysing `parse_quotes_page` is out of scope here because it's not part of the spider's work logic.
The examples use [`scraper`](https://docs.rs/scraper/latest/scraper/) to achieve their objectives.

# Regarding work counters

A default setup will use a global singleton counter and share it among all `WorkPipe` instances.
Normally, there's no reason to override it, as having multiple work counters in one crawler can cause
premature shutdowns or even deadlocks. The counter solves several problems commonly found in crawlers with concurrency.

I hope the following discussion can help you understand the problems, the overall architecture of this crate,
and make better decisions should you choose to not use the default global counter.

## Self-feeding workers

A typical scenario is a worker who self-feeds work, e.g. one that iteratively navigates through pages of results.
You'd kick it off by giving it the URL to the first page, after which it would find the link to the next page and queue it up.
If you used a bare channel, the worker would keep the only remaining `Sender` (assuming the kickoff clone is dropped)
and its work loop manager the only `Receiver`. Thus, the channel would never get closed (because the `Sender` is never dropped)
and the worker would wait on `recv()` indefinitely.

We sidestep this by instead counting how much work is in the pipe. Channels don't provide a way to see how much items are in the buffer,
but we introduce an atomic counter and make sure it's in sync with items going in and out of the channel.
In a simplest case of a self-feeder, the counter can only have three values:
- 1 - if there is work queued and the worker is typically currently processing, or about to start
- 2 - the worker had just queued work on the next page and is about to exit its service function
- 0 - the worker exited the service function but was unable to queue up the next page - the system takes that as
      an indication of no further work and exits the worker's work loop

## Ping-ponging workers

Now imagine there's two pipes and two workers. A can only download pages and B can only process them, meaning
only one of them is ever working at a time (assuming a single-page kick-off and only one "next page" produced per page).
If B is the only one who can find the link to the next page, then A's work counter would deplete to 0 while B is working,
causing A to shutdown prematurely.

The solution is to have the two pipes share the counter. This way, the only scenario in which it goes to 0 is when
there is not only no work for either worker, but also *no possibility* of new work arriving.

That's why, by default, pipes will share the counter:

```rust
let (download_pipe, download_worker) = WorkPipeBuilder::default()
    .build();

let (parse_pipe, parse_worker) = WorkPipeBuilder::default()
    .build(); // Will share counter with the previous
```