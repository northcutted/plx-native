//! Read the moving window of an existing PlayQueue; never replace or take ownership of it.
use super::client::{Client, QueryBuilder};
use super::{models::MediaContainer, QueueRow, ServerId};

fn window_query(id: &str, center: i64, window: i64) -> Option<String> {
    let id = id.parse::<i64>().ok().filter(|id| *id > 0)?;
    if center <= 0 {
        return None;
    }
    // docs/plex-openapi.json: GET /playQueues/{playQueueId}. `center` is an item ID,
    // independent of the server's selected item. `window` is a page size, not a total cap.
    Some(
        QueryBuilder::new(&format!("/playQueues/{id}"))
            .int("own", 0)
            .int("center", center)
            .int("window", window)
            .int("includeBefore", 1)
            .int("includeAfter", 1)
            .build(),
    )
}

fn window_rows(mc: MediaContainer, id: i64, sid: ServerId) -> Option<Vec<QueueRow>> {
    if mc.play_queue_id != id {
        return None;
    }
    Some(
        mc.metadata
            .into_iter()
            .map(|row| QueueRow::of(row, sid))
            .collect(),
    )
}

impl Client {
    pub(super) fn queue_container(&self, id: i64, center: i64) -> Option<MediaContainer> {
        let query = window_query(&id.to_string(), center, 50)?;
        self.get_json(&query).filter(|mc| mc.play_queue_id == id)
    }

    /// GET the existing queue around an item, without transferring ownership or changing the
    /// selected item. The caller validates that the requested identity exists in the response.
    pub fn play_queue_window(&self, id: &str, center: i64) -> Option<Vec<QueueRow>> {
        let query = window_query(id, center, 50)?;
        window_rows(self.get_json(&query)?, id.parse().ok()?, self.id())
    }

    /// On a long seek, allow PMS to return a page around the absolute position in one read.
    /// A server that ignores listing offsets still gets a normal wider, item-centred request.
    pub(super) fn playlist_seek_window(&self, id: i64, anchor: i64, distance: usize, seek_to: Option<usize>)
        -> Option<(Option<usize>, Vec<QueueRow>)> {
        use super::paging::{paged_path, PageReq};
        let window = distance.saturating_mul(2).clamp(50, 200) as i64;
        let centered = window_query(&id.to_string(), anchor, window)?;
        if let Some(start) = seek_to.and_then(|at| at.checked_sub(25)).filter(|start| *start > 0 && distance > 200) {
            let paged = paged_path(&centered, PageReq { start, size: 50 });
            if let Some(mc) = self.get_json(&paged).filter(|page| page.play_queue_id == id) {
                let base = (usize::try_from(mc.offset).ok() == Some(start)).then_some(start);
                if base.is_some() || mc.metadata.iter().any(|row| row.play_queue_item_id == anchor) {
                    return Some((base, window_rows(mc, id, self.id())?));
                }
            }
        }
        Some((None, window_rows(self.get_json(&centered)?, id, self.id())?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_window_queue_window_uses_item_identity_and_does_not_take_ownership() {
        let query = window_query("7", 12, 50).unwrap();
        assert_eq!(
            query,
            "/playQueues/7?own=0&center=12&window=50&includeBefore=1&includeAfter=1"
        );
        assert!(window_query("7/other", 12, 50).is_none());
        assert!(window_query("7", 0, 50).is_none());
        let mc: super::super::models::Envelope = serde_json::from_str(
            r#"{"MediaContainer":{
            "playQueueID":7,"playQueueSelectedItemID":11,"Metadata":[
            {"playQueueItemID":11,"ratingKey":"same","type":"episode"},
            {"playQueueItemID":12,"ratingKey":"same","type":"episode"},
            {"playQueueItemID":13,"ratingKey":"third","type":"episode"}]}}"#,
        )
        .unwrap();
        let sid = ServerId::from_raw(2);
        let rows = window_rows(mc.media_container, 7, sid).unwrap();
        assert_eq!(
            rows.iter().map(|r| r.item_id).collect::<Vec<_>>(),
            vec![11, 12, 13]
        );
        assert!(rows.iter().all(|r| r.sid == sid));
        assert!(window_rows(MediaContainer::default(), 7, sid).is_none());
    }
}
