use crate::detections::{configs, detection::EvtxRecordInfo, utils};
use std::collections::HashMap;

#[derive(Debug)]
pub struct EventStatistics {
    pub total: usize,
    pub filepath: String,
    pub start_time: String,
    pub end_time: String,
    pub stats_list: HashMap<String, usize>,
    pub stats_login_list: HashMap<String, [usize; 2]>,
}
/**
* Windows Event Logの統計情報を出力する
*/
impl EventStatistics {
    pub fn new(
        total: usize,
        filepath: String,
        start_time: String,
        end_time: String,
        stats_list: HashMap<String, usize>,
        stats_login_list: HashMap<String, [usize; 2]>,
    ) -> EventStatistics {
        return EventStatistics {
            total,
            filepath,
            start_time,
            end_time,
            stats_list,
            stats_login_list,
        };
    }

    pub fn start(&mut self, records: &Vec<EvtxRecordInfo>) {
        // 引数でstatisticsオプションが指定されている時だけ、統計情報を出力する。
        if !configs::CONFIG
            .read()
            .unwrap()
            .args
            .is_present("statistics")
        {
            return;
        }

        //let mut filesize = 0;
        // _recordsから、EventIDを取り出す。
        self.stats_time_cnt(records);

        // EventIDで集計
        //let evtstat_map = HashMap::new();
        self.stats_eventid(records);

        // ユーザ毎のログイン成功・失敗の集計
        self.stats_login_eventid(records);
    }

    fn stats_time_cnt(&mut self, records: &Vec<EvtxRecordInfo>) {
        if records.len() == 0 {
            return;
        }
        self.filepath = records[0].evtx_filepath.as_str().to_owned();
        // sortしなくてもイベントログのTimeframeを取得できるように修正しました。
        // sortしないことにより計算量が改善されています。
        // もうちょっと感じに書けるといえば書けます。
        for record in records.iter() {
            let evttime = utils::get_event_value(
                &"Event.System.TimeCreated_attributes.SystemTime".to_string(),
                &record.record,
            )
            .and_then(|evt_value| {
                return Option::Some(evt_value.to_string());
            });
            if evttime.is_none() {
                continue;
            }

            let evttime = evttime.unwrap();
            if self.start_time.len() == 0 || evttime < self.start_time {
                self.start_time = evttime.to_string();
            }
            if self.end_time.len() == 0 || evttime > self.end_time {
                self.end_time = evttime;
            }
        }
        self.total += records.len();
    }

    // EventIDで集計
    fn stats_eventid(&mut self, records: &Vec<EvtxRecordInfo>) {
        //        let mut evtstat_map = HashMap::new();
        for record in records.iter() {
            let evtid = utils::get_event_value(&"EventID".to_string(), &record.record);
            if evtid.is_none() {
                continue;
            }

            let idnum = evtid.unwrap();
            let count: &mut usize = self.stats_list.entry(idnum.to_string()).or_insert(0);
            *count += 1;
        }
        //        return evtstat_map;
    }
    // ユーザ毎にEventID(ログイン成功(4624)・失敗(4634))で集計
    fn stats_login_eventid(&mut self, records: &Vec<EvtxRecordInfo>) {
        // let mut evtstat_map = HashMap::new();
        for record in records.iter() {
            let evtid = utils::get_event_value(&"EventID".to_string(), &record.record);
            let username = utils::get_event_value(&"TargetUserName".to_string(), &record.record);
            if evtid.is_none() {
                continue;
            }
            let idnum = evtid.unwrap();
            let countlist: [usize; 2] = [0, 0];
            if idnum.to_string() == "4624".to_string() {
                let count: &mut [usize; 2] = self
                    .stats_login_list
                    .entry(username.unwrap().to_string())
                    .or_insert(countlist);
                count[0] += 1;
                //println!("ok:{:?},{:?}",count, username);
            } else if idnum.to_string() == "4634".to_string() {
                let count: &mut [usize; 2] = self
                    .stats_login_list
                    .entry(username.unwrap().to_string())
                    .or_insert(countlist);
                count[1] += 1;
                //println!("ng:{:?},{:?}",count, username);
            }
        }
    }
}
