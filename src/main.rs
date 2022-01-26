extern crate serde;
extern crate serde_derive;

use chrono::Datelike;
use chrono::{DateTime, Local};
use evtx::{EvtxParser, ParserSettings};
use hayabusa::detections::detection::{self, EvtxRecordInfo};
use hayabusa::detections::print::AlertMessage;
use hayabusa::detections::print::ERROR_LOG_PATH;
use hayabusa::detections::print::ERROR_LOG_STACK;
use hayabusa::detections::print::LOGONSUMMARY_FLAG;
use hayabusa::detections::print::QUIET_ERRORS_FLAG;
use hayabusa::detections::print::STATISTICS_FLAG;
use hayabusa::detections::rule::{get_detection_keys, RuleNode};
use hayabusa::filter;
use hayabusa::omikuji::Omikuji;
use hayabusa::{afterfact::after_fact, detections::utils};
use hayabusa::{detections::configs, timeline::timeline::Timeline};
use hhmmss::Hhmmss;
use pbr::ProgressBar;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::io::BufWriter;
use std::path::Path;
use std::sync::Arc;
use std::{
    fs::{self, File},
    path::PathBuf,
    vec,
};
use tokio::runtime::Runtime;
use tokio::spawn;
use tokio::task::JoinHandle;

// 一度にtimelineやdetectionを実行する行数
const MAX_DETECT_RECORDS: usize = 5000;

fn main() {
    let mut app = App::new();
    app.exec();
    app.rt.shutdown_background();
}

pub struct App {
    rt: Runtime,
    rule_keys: Vec<String>,
}

impl App {
    pub fn new() -> App {
        return App {
            rt: utils::create_tokio_runtime(),
            rule_keys: Vec::new(),
        };
    }

    fn exec(&mut self) {
        let analysis_start_time: DateTime<Local> = Local::now();
        if !configs::CONFIG.read().unwrap().args.is_present("quiet") {
            self.output_logo();
            println!("");
            self.output_eggs(&format!(
                "{:02}/{:02}",
                &analysis_start_time.month().to_owned(),
                &analysis_start_time.day().to_owned()
            ));
        }
        if !Path::new("./config").exists() {
            AlertMessage::alert(
                &mut BufWriter::new(std::io::stderr().lock()),
                &"Hayabusa could not find the config directory.\nPlease run it from the Hayabusa root directory.\nExample: ./bin/hayabusa-1.0.0-windows-x64.exe".to_string()
            )
            .ok();
            return;
        }
        if configs::CONFIG.read().unwrap().args.args.len() == 0 {
            println!(
                "{}",
                configs::CONFIG.read().unwrap().args.usage().to_string()
            );
            println!("");
            return;
        }
        if let Some(csv_path) = configs::CONFIG.read().unwrap().args.value_of("output") {
            if Path::new(csv_path).exists() {
                AlertMessage::alert(
                    &mut BufWriter::new(std::io::stderr().lock()),
                    &format!(
                        " The file {} already exists. Please specify a different filename.",
                        csv_path
                    ),
                )
                .ok();
                return;
            }
        }
        if *STATISTICS_FLAG {
            println!("Generating Event ID Statistics");
            println!("");
        }
        if *LOGONSUMMARY_FLAG {
            println!("Generating Logons Summary");
            println!("");
        }
        if let Some(filepath) = configs::CONFIG.read().unwrap().args.value_of("filepath") {
            if !filepath.ends_with(".evtx") {
                AlertMessage::alert(
                    &mut BufWriter::new(std::io::stderr().lock()),
                    &"--filepath only accepts .evtx files.".to_string(),
                )
                .ok();
                return;
            }
            self.analysis_files(vec![PathBuf::from(filepath)]);
        } else if let Some(directory) = configs::CONFIG.read().unwrap().args.value_of("directory") {
            let evtx_files = self.collect_evtxfiles(&directory);
            if evtx_files.len() == 0 {
                AlertMessage::alert(
                    &mut BufWriter::new(std::io::stderr().lock()),
                    &"No .evtx files were found.".to_string(),
                )
                .ok();
                return;
            }
            self.analysis_files(evtx_files);
        } else if configs::CONFIG
            .read()
            .unwrap()
            .args
            .is_present("contributors")
        {
            self.print_contributors();
            return;
        }
        let analysis_end_time: DateTime<Local> = Local::now();
        let analysis_duration = analysis_end_time.signed_duration_since(analysis_start_time);
        println!("");
        println!("Elapsed Time: {}", &analysis_duration.hhmmssxxx());
        println!("");

        // Qオプションを付けた場合もしくはパースのエラーがない場合はerrorのstackが9となるのでエラーログファイル自体が生成されない。
        if ERROR_LOG_STACK.lock().unwrap().len() > 0 {
            AlertMessage::create_error_log(ERROR_LOG_PATH.to_string());
        }
    }

    fn collect_evtxfiles(&self, dirpath: &str) -> Vec<PathBuf> {
        let entries = fs::read_dir(dirpath);
        if entries.is_err() {
            let errmsg = format!("{}", entries.unwrap_err());
            if configs::CONFIG.read().unwrap().args.is_present("verbose") {
                AlertMessage::alert(&mut BufWriter::new(std::io::stderr().lock()), &errmsg).ok();
            }
            if !*QUIET_ERRORS_FLAG {
                ERROR_LOG_STACK
                    .lock()
                    .unwrap()
                    .push(format!("[ERROR] {}", errmsg));
            }
            return vec![];
        }

        let mut ret = vec![];
        for e in entries.unwrap() {
            if e.is_err() {
                continue;
            }

            let path = e.unwrap().path();
            if path.is_dir() {
                path.to_str().and_then(|path_str| {
                    let subdir_ret = self.collect_evtxfiles(path_str);
                    ret.extend(subdir_ret);
                    return Option::Some(());
                });
            } else {
                let path_str = path.to_str().unwrap_or("");
                if path_str.ends_with(".evtx") {
                    ret.push(path);
                }
            }
        }

        return ret;
    }

    fn print_contributors(&self) {
        match fs::read_to_string("./contributors.txt") {
            Ok(contents) => println!("{}", contents),
            Err(err) => {
                AlertMessage::alert(
                    &mut BufWriter::new(std::io::stderr().lock()),
                    &format!("{}", err),
                )
                .ok();
            }
        }
    }

    fn analysis_files(&mut self, evtx_files: Vec<PathBuf>) {
        let level = configs::CONFIG
            .read()
            .unwrap()
            .args
            .value_of("min-level")
            .unwrap_or("informational")
            .to_uppercase();
        println!("Analyzing event files: {:?}", evtx_files.len());

        let rule_files = detection::Detection::parse_rule_files(
            level,
            configs::CONFIG.read().unwrap().args.value_of("rules"),
            &filter::exclude_ids(),
        );
        let mut pb = ProgressBar::new(evtx_files.len() as u64);
        pb.show_speed = false;
        self.rule_keys = self.get_all_keys(&rule_files);
        let mut detection = detection::Detection::new(rule_files);
        for evtx_file in evtx_files {
            if configs::CONFIG.read().unwrap().args.is_present("verbose") {
                println!("Checking target evtx FilePath: {:?}", &evtx_file);
            }
            detection = self.analysis_file(evtx_file, detection);
            pb.inc();
        }
        detection.add_aggcondition_msges(&self.rt);
        if !(*STATISTICS_FLAG || *LOGONSUMMARY_FLAG) {
            after_fact();
        }
    }

    // Windowsイベントログファイルを1ファイル分解析する。
    fn analysis_file(
        &self,
        evtx_filepath: PathBuf,
        mut detection: detection::Detection,
    ) -> detection::Detection {
        let path = evtx_filepath.display();
        let parser = self.evtx_to_jsons(evtx_filepath.clone());
        if parser.is_none() {
            return detection;
        }

        let mut tl = Timeline::new();
        let mut parser = parser.unwrap();
        let mut records = parser.records_json_value();

        loop {
            let mut records_per_detect = vec![];
            while records_per_detect.len() < MAX_DETECT_RECORDS {
                // パースに失敗している場合、エラーメッセージを出力
                let next_rec = records.next();
                if next_rec.is_none() {
                    break;
                }

                let record_result = next_rec.unwrap();
                if record_result.is_err() {
                    let evtx_filepath = &path;
                    let errmsg = format!(
                        "Failed to parse event file. EventFile:{} Error:{}",
                        evtx_filepath,
                        record_result.unwrap_err()
                    );
                    if configs::CONFIG.read().unwrap().args.is_present("verbose") {
                        AlertMessage::alert(&mut BufWriter::new(std::io::stderr().lock()), &errmsg)
                            .ok();
                    }
                    if !*QUIET_ERRORS_FLAG {
                        ERROR_LOG_STACK
                            .lock()
                            .unwrap()
                            .push(format!("[ERROR] {}", errmsg));
                    }
                    continue;
                }

                // target_eventids.txtでフィルタする。
                let data = record_result.unwrap().data;
                if self._is_target_event_id(&data) == false {
                    continue;
                }

                // EvtxRecordInfo構造体に変更
                records_per_detect.push(data);
            }
            if records_per_detect.len() == 0 {
                break;
            }

            let records_per_detect = self.rt.block_on(App::create_rec_infos(
                records_per_detect,
                &path,
                self.rule_keys.clone(),
            ));

            // timeline機能の実行
            tl.start(&records_per_detect);

            if !(*STATISTICS_FLAG || *LOGONSUMMARY_FLAG) {
                // ruleファイルの検知
                detection = detection.start(&self.rt, records_per_detect);
            }
        }

        tl.tm_evt_stats_dsp_msg();
        tl.tm_logon_stats_dsp_msg();

        return detection;
    }

    async fn create_rec_infos(
        records_per_detect: Vec<Value>,
        path: &dyn Display,
        rule_keys: Vec<String>,
    ) -> Vec<EvtxRecordInfo> {
        let path = Arc::new(path.to_string());
        let rule_keys = Arc::new(rule_keys);
        let threads: Vec<JoinHandle<EvtxRecordInfo>> = records_per_detect
            .into_iter()
            .map(|rec| {
                let arc_rule_keys = Arc::clone(&rule_keys);
                let arc_path = Arc::clone(&path);
                return spawn(async move {
                    let rec_info =
                        utils::create_rec_info(rec, arc_path.to_string(), &arc_rule_keys);
                    return rec_info;
                });
            })
            .collect();

        let mut ret = vec![];
        for thread in threads.into_iter() {
            ret.push(thread.await.unwrap());
        }

        return ret;
    }

    fn get_all_keys(&self, rules: &Vec<RuleNode>) -> Vec<String> {
        let mut key_set = HashSet::new();
        for rule in rules {
            let keys = get_detection_keys(rule);
            key_set.extend(keys);
        }

        let ret: Vec<String> = key_set.into_iter().collect();
        return ret;
    }

    // target_eventids.txtの設定を元にフィルタする。
    fn _is_target_event_id(&self, data: &Value) -> bool {
        let eventid = utils::get_event_value(&utils::get_event_id_key(), data);
        if eventid.is_none() {
            return true;
        }

        return match eventid.unwrap() {
            Value::String(s) => utils::is_target_event_id(s),
            Value::Number(n) => utils::is_target_event_id(&n.to_string()),
            _ => true, // レコードからEventIdが取得できない場合は、特にフィルタしない
        };
    }

    fn evtx_to_jsons(&self, evtx_filepath: PathBuf) -> Option<EvtxParser<File>> {
        match EvtxParser::from_path(evtx_filepath) {
            Ok(evtx_parser) => {
                // parserのデフォルト設定を変更
                let mut parse_config = ParserSettings::default();
                parse_config = parse_config.separate_json_attributes(true); // XMLのattributeをJSONに変換する時のルールを設定
                parse_config = parse_config.num_threads(0); // 設定しないと遅かったので、設定しておく。

                let evtx_parser = evtx_parser.with_configuration(parse_config);
                return Option::Some(evtx_parser);
            }
            Err(e) => {
                eprintln!("{}", e);
                return Option::None;
            }
        }
    }

    fn _output_with_omikuji(&self, omikuji: Omikuji) {
        let fp = &format!("art/omikuji/{}", omikuji);
        let content = fs::read_to_string(fp).unwrap();
        println!("{}", content);
    }

    /// output logo
    fn output_logo(&self) {
        let fp = &format!("art/logo.txt");
        let content = fs::read_to_string(fp).unwrap_or("".to_owned());
        println!("{}", content);
    }

    /// output easter egg arts
    fn output_eggs(&self, exec_datestr: &str) {
        let mut eggs: HashMap<&str, &str> = HashMap::new();
        eggs.insert("01/01", "art/happynewyear.txt");
        eggs.insert("02/22", "art/ninja.txt");
        eggs.insert("08/08", "art/takoyaki.txt");
        eggs.insert("12/25", "art/christmas.txt");

        match eggs.get(exec_datestr) {
            None => {}
            Some(path) => {
                let content = fs::read_to_string(path).unwrap_or("".to_owned());
                println!("{}", content);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::App;

    #[test]
    fn test_collect_evtxfiles() {
        let app = App::new();
        let files = app.collect_evtxfiles("test_files/evtx");
        assert_eq!(3, files.len());

        files.iter().for_each(|file| {
            let is_contains = &vec!["test1.evtx", "test2.evtx", "testtest4.evtx"]
                .into_iter()
                .any(|filepath_str| {
                    return file.file_name().unwrap().to_str().unwrap_or("") == filepath_str;
                });
            assert_eq!(is_contains, &true);
        })
    }
}
