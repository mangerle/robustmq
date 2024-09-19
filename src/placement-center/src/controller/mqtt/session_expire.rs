// Copyright 2023 RobustMQ Team
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::call_broker::MQTTBrokerCall;
use crate::{
    cache::{mqtt::MqttCacheManager, placement::PlacementCacheManager},
    storage::{
        keys::storage_key_mqtt_session_cluster_prefix, mqtt::lastwill::MQTTLastWillStorage,
        rocksdb::RocksDBEngine, StorageDataWrap,
    },
};
use clients::poll::ClientPool;
use common_base::tools::now_second;
use log::{debug, error};
use metadata_struct::mqtt::session::MQTTSession;
use std::{sync::Arc, time::Duration};
use tokio::time::sleep;

#[derive(Clone, Debug)]
pub struct ExpireLastWill {
    pub client_id: String,
    pub delay_sec: u64,
    pub cluster_name: String,
}

pub struct SessionExpire {
    rocksdb_engine_handler: Arc<RocksDBEngine>,
    mqtt_cache_manager: Arc<MqttCacheManager>,
    placement_cache_manager: Arc<PlacementCacheManager>,
    client_poll: Arc<ClientPool>,
    cluster_name: String,
}

impl SessionExpire {
    pub fn new(
        rocksdb_engine_handler: Arc<RocksDBEngine>,
        mqtt_cache_manager: Arc<MqttCacheManager>,
        placement_cache_manager: Arc<PlacementCacheManager>,
        client_poll: Arc<ClientPool>,
        cluster_name: String,
    ) -> Self {
        return SessionExpire {
            rocksdb_engine_handler,
            mqtt_cache_manager,
            placement_cache_manager,
            client_poll,
            cluster_name,
        };
    }

    pub async fn session_expire(&self) {
        let sessions = self.get_expire_session_list().await;
        if sessions.len() > 0 {
            self.delete_session(sessions);
        }
        sleep(Duration::from_secs(1)).await;
    }

    pub async fn lastwill_expire_send(&self) {
        let lastwill_list = self.get_expire_lastwill_messsage();
        if lastwill_list.len() > 0 {
            debug!("Will message due, list:{:?}", lastwill_list);
            self.send_expire_lastwill_messsage(lastwill_list).await;
        }
        sleep(Duration::from_secs(10)).await;
    }

    async fn get_expire_session_list(&self) -> Vec<MQTTSession> {
        let search_key = storage_key_mqtt_session_cluster_prefix(&self.cluster_name);
        let cf = self.rocksdb_engine_handler.cf_cluster();
        let mut iter = self.rocksdb_engine_handler.db.raw_iterator_cf(cf);
        iter.seek(search_key.clone());
        let mut sessions = Vec::new();
        while iter.valid() {
            let key = iter.key();
            let value = iter.value();

            if key == None || value == None {
                iter.next();
                continue;
            }

            let result_key = match String::from_utf8(key.unwrap().to_vec()) {
                Ok(s) => s,
                Err(_) => {
                    iter.next();
                    continue;
                }
            };

            if !result_key.starts_with(&search_key) {
                break;
            }
            let result_value = value.unwrap();
            let session = match serde_json::from_slice::<StorageDataWrap>(&result_value) {
                Ok(data) => match serde_json::from_slice::<MQTTSession>(&data.data) {
                    Ok(da) => da,
                    Err(e) => {
                        error!(
                            "Session expired, failed to parse Session data, error message :{}",
                            e.to_string()
                        );
                        iter.next();
                        continue;
                    }
                },
                Err(e) => {
                    error!(
                        "Session expired, failed to parse Session data, error message :{}",
                        e.to_string()
                    );
                    iter.next();
                    continue;
                }
            };
            if self.is_session_expire(&session) {
                sessions.push(session);
            }
            iter.next();
        }
        return sessions;
    }

    fn delete_session(&self, sessions: Vec<MQTTSession>) {
        let call = MQTTBrokerCall::new(
            self.cluster_name.clone(),
            self.placement_cache_manager.clone(),
            self.rocksdb_engine_handler.clone(),
            self.client_poll.clone(),
            self.mqtt_cache_manager.clone(),
        );
        tokio::spawn(async move {
            call.delete_sessions(sessions).await;
        });
    }

    fn get_expire_lastwill_messsage(&self) -> Vec<ExpireLastWill> {
        let mut results = Vec::new();
        if let Some(list) = self
            .mqtt_cache_manager
            .expire_last_wills
            .get(&self.cluster_name)
        {
            for (_, lastwill) in list.clone() {
                if self.is_send_last_will(&lastwill) {
                    results.push(lastwill);
                }
            }
        }
        return results;
    }

    async fn send_expire_lastwill_messsage(&self, last_will_list: Vec<ExpireLastWill>) {
        let lastwill_storage = MQTTLastWillStorage::new(self.rocksdb_engine_handler.clone());
        let call = MQTTBrokerCall::new(
            self.cluster_name.clone(),
            self.placement_cache_manager.clone(),
            self.rocksdb_engine_handler.clone(),
            self.client_poll.clone(),
            self.mqtt_cache_manager.clone(),
        );
        for lastwill in last_will_list {
            match lastwill_storage.get(&self.cluster_name, &lastwill.client_id) {
                Ok(Some(data)) => {
                    call.send_last_will_message(lastwill.client_id.clone(), data)
                        .await;
                }
                Ok(None) => {
                    self.mqtt_cache_manager
                        .remove_expire_last_will(&self.cluster_name, &lastwill.client_id);
                }
                Err(e) => {
                    sleep(Duration::from_millis(100)).await;
                    error!("{}", e);
                    continue;
                }
            }
        }
    }

    fn is_session_expire(&self, session: &MQTTSession) -> bool {
        if session.connection_id.is_none() && session.broker_id.is_none() {
            if let Some(distinct_time) = session.distinct_time {
                if now_second() >= (session.session_expiry + distinct_time) {
                    return true;
                }
            }
        }

        return false;
    }

    fn is_send_last_will(&self, lastwill: &ExpireLastWill) -> bool {
        if now_second() >= lastwill.delay_sec {
            return true;
        }
        return false;
    }
}

#[cfg(test)]
mod tests {
    use std::{fs::remove_dir_all, sync::Arc, time::Duration};

    use clients::poll::ClientPool;
    use common_base::{
        config::placement_center::PlacementCenterConfig,
        tools::{now_second, unique_id},
    };
    use metadata_struct::mqtt::session::MQTTSession;
    use tokio::time::sleep;

    use super::SessionExpire;
    use crate::controller::mqtt::session_expire::ExpireLastWill;
    use crate::{
        cache::{mqtt::MqttCacheManager, placement::PlacementCacheManager},
        storage::{mqtt::session::MQTTSessionStorage, rocksdb::RocksDBEngine},
    };

    #[test]
    fn is_session_expire_test() {
        let mut config = PlacementCenterConfig::default();
        config.rocksdb.data_path = format!("/tmp/{}", unique_id());
        config.rocksdb.max_open_files = Some(10);

        let cluster_name = unique_id();
        let rocksdb_engine_handler = Arc::new(RocksDBEngine::new(&config));
        let placement_cache = Arc::new(PlacementCacheManager::new(rocksdb_engine_handler.clone()));
        let mqtt_cache_manager = Arc::new(MqttCacheManager::new(
            rocksdb_engine_handler.clone(),
            placement_cache.clone(),
        ));
        let client_poll = Arc::new(ClientPool::new(10));

        let session_expire = SessionExpire::new(
            rocksdb_engine_handler,
            mqtt_cache_manager,
            placement_cache,
            client_poll,
            cluster_name,
        );

        let mut session = MQTTSession::default();
        session.session_expiry = now_second() - 100;
        session.distinct_time = Some(5);
        assert!(session_expire.is_session_expire(&session));

        let mut session = MQTTSession::default();
        session.broker_id = Some(1);
        session.reconnect_time = Some(now_second());
        session.session_expiry = now_second() + 100;
        session.distinct_time = None;
        assert!(!session_expire.is_session_expire(&session));

        remove_dir_all(config.rocksdb.data_path).unwrap();
    }

    #[tokio::test]
    async fn get_expire_session_list_test() {
        let mut config = PlacementCenterConfig::default();
        config.rocksdb.data_path = format!("/tmp/{}", unique_id());
        config.rocksdb.max_open_files = Some(10);

        let cluster_name = unique_id();
        let rocksdb_engine_handler = Arc::new(RocksDBEngine::new(&config));
        let placement_cache = Arc::new(PlacementCacheManager::new(rocksdb_engine_handler.clone()));
        let mqtt_cache_manager = Arc::new(MqttCacheManager::new(
            rocksdb_engine_handler.clone(),
            placement_cache.clone(),
        ));
        let client_poll = Arc::new(ClientPool::new(10));

        let session_expire = SessionExpire::new(
            rocksdb_engine_handler.clone(),
            mqtt_cache_manager,
            placement_cache,
            client_poll,
            cluster_name.clone(),
        );

        let session_storage = MQTTSessionStorage::new(rocksdb_engine_handler.clone());
        let client_id = unique_id();
        let mut session = MQTTSession::default();

        session.session_expiry = 3;
        session.distinct_time = Some(now_second());

        session.client_id = client_id.clone();
        session_storage
            .save(&cluster_name, &client_id, session)
            .unwrap();

        let start = now_second();
        loop {
            let expire_list = session_expire.get_expire_session_list().await;

            if expire_list.len() > 0 {
                let mut flag = false;
                for st in expire_list {
                    if st.client_id == client_id {
                        flag = true;
                    }
                }

                if flag {
                    break;
                }
            }

            sleep(Duration::from_millis(1000)).await;
        }
        assert_eq!((now_second() - start), 3);

        remove_dir_all(config.rocksdb.data_path).unwrap();
    }

    #[tokio::test]
    async fn is_send_last_will_test() {
        let mut config = PlacementCenterConfig::default();
        config.rocksdb.data_path = format!("/tmp/{}", unique_id());
        config.rocksdb.max_open_files = Some(10);

        let cluster_name = unique_id();
        let rocksdb_engine_handler = Arc::new(RocksDBEngine::new(&config));
        let placement_cache = Arc::new(PlacementCacheManager::new(rocksdb_engine_handler.clone()));
        let mqtt_cache_manager = Arc::new(MqttCacheManager::new(
            rocksdb_engine_handler.clone(),
            placement_cache.clone(),
        ));
        let client_poll = Arc::new(ClientPool::new(10));

        let session_expire = SessionExpire::new(
            rocksdb_engine_handler.clone(),
            mqtt_cache_manager,
            placement_cache,
            client_poll,
            cluster_name.clone(),
        );

        let lastwill = ExpireLastWill {
            client_id: unique_id(),
            delay_sec: now_second() - 3,
            cluster_name: "test1".to_string(),
        };

        assert!(session_expire.is_send_last_will(&lastwill));

        let lastwill = ExpireLastWill {
            client_id: unique_id(),
            delay_sec: now_second() + 3,
            cluster_name: "test1".to_string(),
        };
        assert!(!session_expire.is_send_last_will(&lastwill));

        remove_dir_all(config.rocksdb.data_path).unwrap();
    }

    #[tokio::test]
    async fn get_expire_lastwill_messsage_test() {
        let mut config = PlacementCenterConfig::default();
        config.rocksdb.data_path = format!("/tmp/{}", unique_id());
        config.rocksdb.max_open_files = Some(10);

        let cluster_name = unique_id();
        let rocksdb_engine_handler = Arc::new(RocksDBEngine::new(&config));
        let placement_cache = Arc::new(PlacementCacheManager::new(rocksdb_engine_handler.clone()));
        let mqtt_cache_manager = Arc::new(MqttCacheManager::new(
            rocksdb_engine_handler.clone(),
            placement_cache.clone(),
        ));
        let client_poll = Arc::new(ClientPool::new(10));

        let session_expire = SessionExpire::new(
            rocksdb_engine_handler.clone(),
            mqtt_cache_manager.clone(),
            placement_cache,
            client_poll,
            cluster_name.clone(),
        );

        let client_id = unique_id();
        let expire_last_will = ExpireLastWill {
            client_id: client_id.clone(),
            delay_sec: now_second() + 3,
            cluster_name: cluster_name.clone(),
        };

        mqtt_cache_manager.add_expire_last_will(expire_last_will);

        let start = now_second();
        loop {
            let lastwill_list = session_expire.get_expire_lastwill_messsage();
            if lastwill_list.len() > 0 {
                let mut flag = false;
                for st in lastwill_list {
                    if st.client_id == client_id {
                        flag = true;
                    }
                }

                if flag {
                    break;
                }
            }

            sleep(Duration::from_millis(1000)).await;
        }

        assert_eq!((now_second() - start), 3);

        remove_dir_all(config.rocksdb.data_path).unwrap();
    }
}
