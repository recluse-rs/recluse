use std::{sync::Arc, time::Duration};
use log::debug;
use anyhow::{Context, Result};
use tokio::task::JoinSet;

use recluse::{JsonDownloaderLayer, LeakyBucketRateLimiterLayer, ServiceBuilderReqwestExt, ServiceBuilderUtilsExt, WorkPipeBuilder};

#[allow(dead_code)]
mod api {
    use serde::Deserialize;

    #[derive(Deserialize, Debug)]
    pub struct IndexPage {
        pub count: usize,
        pub next: Option<String>,
        pub previous: Option<String>,
        pub results: Vec<IndexItem>,
    }
    
    /// An item in a collection page.
    #[derive(Deserialize, Debug)]
    pub struct IndexItem {
        pub name: String,
        pub url: String,
    }

    /// A type with a slot number
    #[derive(Deserialize, Debug)]
    pub struct SlottedType {
        pub slot: usize,
        pub r#type: IndexItem,
    }

    /// An ability with a slot number
    #[derive(Deserialize, Debug)]
    pub struct SlottedAbility {
        pub slot: usize,
        pub ability: IndexItem,
    }

    /// A Pokemon
    #[derive(Deserialize, Debug)]
    pub struct Pokemon {
        pub name: String,
        pub types: Vec<SlottedType>,
        pub abilities: Vec<SlottedAbility>,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    colog::init();

    // Create all the pipes
    let (pokemon_index_pipe, pokemon_index_worker) = WorkPipeBuilder::default().build();
    let (pokemon_pipe, pokemon_worker) = WorkPipeBuilder::default().build();

    // The two workers hit the same website, therefore they need to share the rate limiter
    let limiter = Arc::new(leaky_bucket::RateLimiter::builder()
        .initial(1).max(4)
        .refill(1).interval(Duration::from_secs(1))
        .build());

    // Process pages that are indexes of Pokemons
    let pokemon_index_processor = {
        let pokemon_index_pipe = pokemon_index_pipe.clone();
        let pokemon_pipe = pokemon_pipe.clone();

        move |index: api::IndexPage| {
            let pokemon_index_pipe = pokemon_index_pipe.clone();
            let pokemon_pipe = pokemon_pipe.clone();
            
            async move {
                debug!("{:?}", index);
                
                for item in index.results {
                    debug!("{}", item.name);
                    pokemon_pipe.submit_work(item.url).await
                        .context("Submit Pokemon")?;
                }

                if let Some(next) = index.next {
                    pokemon_index_pipe.submit_work(next.to_string()).await
                        .context("Submit next page URL")?;
                }

                anyhow::Ok(())
            }
        }
    };

    // Wrap it into a tower::Service
    let pokemon_index_service = tower::ServiceBuilder::new()
        .layer(LeakyBucketRateLimiterLayer::new(limiter.clone()))
        .map_string_to_reqwest_get()
        .print_and_drop_request_error()
        .layer(JsonDownloaderLayer::<_>::new())
        .service_fn(pokemon_index_processor);

    // Process a single Pokemon page
    let pokemon_processor = {
        async move |pokemon: api::Pokemon| {
            debug!("{:?}", pokemon);

            anyhow::Ok(())
        }
    };

    // Wrap it into a tower::Service
    let pokemon_service = tower::ServiceBuilder::new()
        .layer(LeakyBucketRateLimiterLayer::new(limiter))
        .map_string_to_reqwest_get()
        .print_and_drop_request_error()
        .layer(JsonDownloaderLayer::<_>::new())
        .service_fn(pokemon_processor);

    // Spawn all the workers
    let mut workers = JoinSet::new();

    workers.spawn(async move {
        pokemon_index_worker.work(pokemon_index_service).await
    });

    workers.spawn(async move {
        pokemon_worker.work(pokemon_service).await
    });

    // Prime work with initial index page
    pokemon_index_pipe.submit_work("https://pokeapi.co/api/v2/pokemon/".to_string()).await
        .context("Send first page")?;

    // Wait for workers to finish
    workers.join_all().await;

    Ok(())
}