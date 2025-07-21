use std::{time::Duration, sync::LazyLock};
use log::*;
use anyhow::{Context, Result};
use scraper::{ElementRef, Html, Selector};

use recluse::{downloader::*, print_errors, WorkPipeBuilder};

#[derive(Debug)]
#[allow(dead_code)]
struct Quote {
    pub author: String,
    pub text: String,
    pub tags: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    colog::init();

    // Create a worker and its input pipe for work items
    let (page_pipe, page_worker) = WorkPipeBuilder::new().build();

    // Create the core function that processes the downloaded pages
    let page_processor = {
        // This cloning gymnastics is, unfortunately, required due to page_pipe being used again later to prime the queue with work
        let page_pipe = page_pipe.clone();
        
        // We accept a string containing the raw HTML of the page
        move |page_body: String| {
            let page_pipe = page_pipe.clone();
            
            async move {
                // Parse the page in search of content of interest
                let (quotes, next_page_url) = parse_quotes_page(page_body)
                    .context("Could not parse quotes page")?;

                // Just print out the quotes.
                // In a real program, you'd send them to another pipe, typically leading to a database writer.
                debug!("{:?}", quotes);

                // Enqueue more work (next page), if found.
                // This is the moment where the work counter comes to play.
                // It's incremented when submitting work and decremented upon exiting this function.
                // Therefore this worker can self-feed after the kick-off, but will stop when there's no more work.
                if let Some(next_page_request) = next_page_url {
                    page_pipe.submit_work(next_page_request).await
                        .context("Queue next page request")?;
                }
                
                anyhow::Ok(())
            }
        }
    };

    // Wrap it in a service with rate limiting.
    // This is where other layers like retries, duplicate removal etc. can go.
    let quotes_page_parser_service = tower::ServiceBuilder::new()
        // This maps Strings to GET reqwest::Request objects
        .map_request(string_to_get_reqwest)
        // Any errors in the mapping are printed by the print_errors function and removed by filter
        .filter(print_errors)
        .rate_limit(1, Duration::from_secs(1))
        // This layer executes requests and gives your service a raw HTML string,
        // so that you don't have to muck around with HTTP clients yourself.
        .layer(BodyDownloaderLayer)
        .service_fn(page_processor);
    
    // Spawn the worker for this pipe
    let worker = tokio::spawn(async move {
        page_worker.work(quotes_page_parser_service).await
    });

    // Prime the work queue with front page
    page_pipe.submit_work("https://quotes.toscrape.com/".to_string()).await
        .context("Sent initial request")?;

    // Wait until worker is done
    tokio::join!(worker).0??;

    Ok(())
}

// Pre-"compile" selectors as they will be re-used many times
static QUOTE_SELECTOR: LazyLock<Selector> = LazyLock::new(|| Selector::parse(".quote")
    .expect("Quote selector should be valid"));

static AUTHOR_SELECTOR: LazyLock<Selector> = LazyLock::new(|| Selector::parse(".author")
    .expect("Author selector should be valid"));

static TEXT_SELECTOR: LazyLock<Selector> = LazyLock::new(|| Selector::parse(".text")
    .expect("Text selector should be valid"));

static TAG_SELECTOR: LazyLock<Selector> = LazyLock::new(|| Selector::parse(".tags .tag")
    .expect("Tag selector should be valid"));

static  NEXT_PAGE_SELECTOR: LazyLock<Selector> = LazyLock::new(|| Selector::parse(".pager .next a")
    .expect("Next page selector should be valid"));

/// Search the body of a page in search of quotes (may be empty) and find the next page (may be None).
fn parse_quotes_page(body: String) -> Result<(Vec<Quote>, Option<String>)> {
    let document = Html::parse_document(&body);

    // Parse the quotes on the current page
    let mut quotes = vec![];
    for quote_element in document.select(&QUOTE_SELECTOR) {
        match parse_quote_element(quote_element) {
            Ok(quote) => quotes.push(quote),
            Err(why) => warn!("Error parsing quote: {}", why),
        }
    }

    // Look for the next page link
    let next_page_href = document.select(&NEXT_PAGE_SELECTOR).next()
        .and_then(|elem| elem.attr("href"))
        .map(|s| s.to_string());

    if let Some(next_page_href) = next_page_href {
        Ok((quotes, Some(format!("https://quotes.toscrape.com{}", next_page_href))))
    } else {
        Ok((quotes, None))
    }
}

// Parse a single quote element.
fn parse_quote_element(element: ElementRef) -> Result<Quote> {
    let author = element.select(&AUTHOR_SELECTOR).next()
        .context("Find author element")?
        .text().next()
        .context("Find author text")?
        .to_string();
    
    let text = element.select(&TEXT_SELECTOR).next()
        .context("Find text element")?
        .text().next()
        .context("Find text text")?
        .to_string();
    
    let mut tags = vec![];
    for tag_element in element.select(&TAG_SELECTOR) {
        if let Some(tag) = tag_element.text().next() {
            tags.push(tag.to_string());
        } else {
            warn!("Error extracting tag");
        }
    }

    Ok(Quote { author, text, tags })
}