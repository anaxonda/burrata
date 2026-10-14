pub mod beatport;
pub mod traxsource;
use crate::models::{Chart, Genre, Preview, Track};
use async_trait::async_trait;

#[async_trait]
pub trait ChartProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn list_genres(&self) -> Vec<Genre>;
    async fn get_chart(&self, url: &str) -> Result<Chart, String>;
}
#[async_trait]
pub trait SearchProvider: Send + Sync {
    async fn search_tracks(&self, query: &str) -> Result<Vec<Track>, String>;
    async fn get_track_details(&self, track: &Track) -> Result<Track, String>;
}
#[async_trait]
pub trait PreviewProvider: Send + Sync {
    async fn resolve_preview(&self, track: &Track) -> Result<Preview, String>;
}
pub trait CatalogProvider: ChartProvider + SearchProvider + PreviewProvider {}
impl<T: ChartProvider + SearchProvider + PreviewProvider> CatalogProvider for T {}

pub fn provider(id: &str) -> Result<Box<dyn CatalogProvider>, String> {
    match id {
        "beatport" => Ok(Box::new(beatport::Beatport::new())),
        "traxsource" => Ok(Box::new(traxsource::Traxsource::new())),
        _ => Err(format!("Unsupported chart provider: {id}")),
    }
}
