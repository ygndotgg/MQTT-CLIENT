#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionEntry {
    pub filter: String,
    pub clients: Vec<usize>,
}

#[derive(Debug, Default)]
pub struct Router {
    entries: Vec<SubscriptionEntry>,
}

fn topic_matches(filter: &str, topic: &str) -> bool {
    let filter_levels: Vec<&str> = filter.split('/').collect();
    let topic_levels: Vec<&str> = topic.split('/').collect();
    let mut i = 0;

    while i < filter_levels.len() {
        let f = filter_levels[i];

        if f == "#" {
            return i == filter_levels.len() - 1;
        }

        if i >= topic_levels.len() {
            return false;
        }

        let t = topic_levels[i];
        if f != "+" && f != t {
            return false;
        }

        i += 1;
    }

    i == topic_levels.len()
}

impl Router {
    pub fn add_subscription(&mut self, client_id: usize, filter: String) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.filter == filter) {
            if !entry.clients.contains(&client_id) {
                entry.clients.push(client_id);
            }
            return;
        }

        self.entries.push(SubscriptionEntry {
            filter,
            clients: vec![client_id],
        });
    }

    pub fn remove_subscription(&mut self, client_id: usize, filter: &str) {
        for entry in &mut self.entries {
            if entry.filter == filter {
                entry.clients.retain(|id| *id != client_id);
            }
        }

        self.entries.retain(|entry| !entry.clients.is_empty());
    }

    pub fn matching_clients(&self, topic: &str) -> Vec<usize> {
        let mut clients = Vec::new();

        for entry in &self.entries {
            if topic_matches(&entry.filter, topic) {
                for client_id in &entry.clients {
                    if !clients.contains(client_id) {
                        clients.push(*client_id);
                    }
                }
            }
        }

        clients
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn router_add_subscription_tracks_multiple_clients() {
        let mut router = Router::default();

        router.add_subscription(1, "sensor/+".to_string());
        router.add_subscription(2, "sensor/+".to_string());

        assert_eq!(router.matching_clients("sensor/temp"), vec![1, 2]);
    }

    #[test]
    fn router_add_subscription_deduplicates_client() {
        let mut router = Router::default();

        router.add_subscription(1, "sensor/+".into());
        router.add_subscription(1, "sensor/+".into());

        assert_eq!(router.matching_clients("sensor/temp"), vec![1]);
    }

    #[test]
    fn router_remove_subscription_drops_empty_entry() {
        let mut router = Router::default();

        router.add_subscription(1, "sensor/+".into());
        router.remove_subscription(1, "sensor/+");

        assert!(router.matching_clients("sensor/temp").is_empty());
    }

    #[test]
    fn router_matching_clients_exact_and_broadcast() {
        let mut router = Router::default();

        router.add_subscription(1, "sensor/temp".into());
        router.add_subscription(2, "sensor/temp".into());

        assert_eq!(router.matching_clients("sensor/temp"), vec![1, 2]);
    }

    #[test]
    fn router_matching_clients_single_level_wildcard() {
        let mut router = Router::default();

        router.add_subscription(1, "sensor/+".into());

        assert_eq!(router.matching_clients("sensor/temp"), vec![1]);
        assert!(router.matching_clients("sensor/temp/outdoor").is_empty());
    }

    #[test]
    fn router_matching_clients_multi_level_wildcard() {
        let mut router = Router::default();

        router.add_subscription(1, "alerts/#".into());

        assert_eq!(router.matching_clients("alerts"), vec![1]);
        assert_eq!(router.matching_clients("alerts/high/cpu"), vec![1]);
    }
}
