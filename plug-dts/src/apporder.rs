use crate::accesstoken::AccessToken;
use crate::appauthorise::AppAuthorise;
use crate::config::WebConfig;
use crate::prox::post;
use chrono::{Local, NaiveDateTime};
use lane::err_info;
use lane_mysql::{DBValue, Table};
use md5;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Error;

//const DEFAULT_DTS_URL: &'static str = "http://127.0.0.1:8090/Route.axd";
pub struct AppOrder {
    pub web: WebConfig,
    pub fk_id: u64,
    pub fk_flag: u32,
    pub user_name: String,
    pub access_token: String,
}
/// 基于Table的实现
impl Table for AppOrder {
    // 实现表名
    fn get_table_name(&self) -> String {
        "pak_customerapp".to_owned()
    }
}
impl AppOrder {
    /**
     * 构造函数
     */
    pub fn new(web: &WebConfig, _fk_id: u64, _fk_flag: u32, _user_name: &str) -> Self {
        AppOrder {
            web: web.clone(),
            fk_id: _fk_id,
            fk_flag: _fk_flag,
            user_name: _user_name.to_owned(),
            access_token: AccessToken::new(_fk_id, _fk_flag, web.db_id, _user_name)
                .to_token_string(),
        }
    }
     /**
     * 获得版本对应的供应商数据
     */
    pub fn get_version_count(&self, version_app: i32) ->u64 {
        let sql = format!(
            r#"
        SELECT count(0) as co
FROM pak_customerapp AS a
JOIN sup_supplier AS b
WHERE a.fkid=b.id AND a.fkflag=2 AND b.expireTime>NOW() AND appid={};
        "#,
            version_app
        );
        let pool = self.get_pool();
        let res: Vec<u64> = pool
            .prep_exec(sql, ())
            .map(|result| {
                result
                    .map(|x| x.unwrap())
                    .map(|row| {
                        row["co"].to_u64()
                    })
                    .collect()
            })
            .unwrap();
        if res.len()>0{
            res[0]
        }
        else{
            0
        }
    }
    /**
     * 获得版本对应的供应商数据
     */
    pub fn get_list_version(&self, version_app: i32,page_size:u64,page_index:u64) ->Result<Vec<(u64, i32, String)>,Error> {
        let sql = format!(
            r#"
        SELECT a.FKId,a.FKFlag,2 AS RunWay,b.Name AS CompanyName
FROM pak_customerapp AS a
JOIN sup_supplier AS b
WHERE a.fkid=b.id AND a.fkflag=2 AND b.expireTime>NOW() AND appid={} limit {},{};
        "#,
            version_app,page_index*page_size,page_size
        );
        let pool = self.get_pool();
        let res: Vec<(u64, i32, String)> = pool
            .prep_exec(sql, ())
            .map(|result| {
                result
                    .map(|x| x.unwrap())
                    .map(|row| {
                        (
                            row["FKId"].to_u64(),
                            row["FKFlag"].to_i32(),
                            DBValue::to_string(&row["CompanyName"], false),
                        )
                    })
                    .collect()
            })
            .unwrap();
        return Ok(res);
    }
    /**
     * 批量插入订单数据
     */
    pub fn batch_insert_order(&self,page_size:u64,page_index:u64, version_app: i32, app_id: i32, app_name: &str, content: &str) {
        //1、查询该版本下的供应商
        let decorate_list = self.decorate_list(page_size,page_index, version_app, app_id, app_name, content);
        //2、同步dts中
        let x: Result<(Vec<(u64, u32, i64)>, Vec<(u64, String)>), std::io::Error> =
            self.send_order_submit(decorate_list);
        //3、支付回调
        let y = match x {
            Ok(s) => {
                self.send_order_callback(s.0)
            }
            Err(e) => Err(err_info!(format!("{:?}", e))),
        };
    }
    /**
     * 装修数据
     */
    pub fn decorate_list(
        &self,
        page_size: u64,
        page_index: u64,
        version_app: i32,
        app_id: i32,
        app_name: &str,
        content: &str,
    ) -> Vec<HashMap<String, String>> {
        let result_list = self.get_list_version( version_app,page_size,page_index);
        let decorate_list = match result_list {
            Ok(list) => {
                let mut vec_list = Vec::new();
                for item in list {
                    let data = self.get_send_data(item, version_app, app_id, app_name, content);
                    vec_list.push(data);
                }
                Ok(vec_list)
            }
            Err(e) => Err(err_info!(format!("{}", e))),
        };
        match decorate_list {
            Ok(list) => list,
            Err(e) => Vec::new(),
        }
    }
    /**
     * 发送订单提交
     */
    fn send_order_submit(
        &self,
        list: Vec<HashMap<String, String>>,
    ) -> Result<(Vec<(u64, u32, i64)>, Vec<(u64, String)>), Error> {
        let mut error_list: Vec<(u64, String)> = Vec::new();
        let mut success_list: Vec<(u64, u32, i64)> = Vec::new();
        for item in list {
            let mut dic = item.clone();
            dic.insert("Data".to_owned(), json::stringify(item.clone()));

            let res = self.post_data(&dic, "aus.package.app.submit");
            let fkid = item.get("FKId").unwrap().parse::<u64>().unwrap();
            let fkflag = item.get("FKFlag").unwrap().parse::<u32>().unwrap();
            let result = self.parse_submit_content(res);
            match result {
                Ok(s) => {
                    println!("订单提交成功，订单号：{:?},fkid={:?},fkflag={:?}", s, fkid, fkflag);
                    success_list.push((fkid, fkflag, s));
                }
                Err(e) => {
                    println!("订单提交失败，error：{:?},fkid={:?},fkflag={:?}", e, fkid, fkflag);
                    error_list.push((fkid, format!("{:?}", e)));
                }
            }
        }
        Ok((success_list, error_list))
    }
    /**
     * 发送数据
     */
    fn get_send_data(
        &self,
        item: (u64, i32, String),
        version_app: i32,
        app_id: i32,
        app_name: &str,
        content: &str,
    ) -> HashMap<String, String> {
        let mut data = HashMap::new();
        let name = item.2;
        let fkid = item.0;
        let fkflag = item.1;
        data.insert("Flag".to_owned(), format!("Upgrade_Plug_{}", app_name));
        data.insert("AppId".to_owned(), format!("{}", app_id));
        data.insert("AppliedId".to_owned(), "0".to_owned());
        data.insert("Content".to_owned(), content.to_owned());
        data.insert("ReceiveFKId".to_owned(), format!("{}", self.fk_id));
        data.insert("ReceiveFKFlag".to_owned(), format!("{}", self.fk_flag));
        data.insert(
            "Remark".to_owned(),
            format!(
                "{}手动批量更新【操作人：任我行科技销售中心;IP=127.0.0.1】",
                content
            ),
        );
        data.insert("RunWay".to_owned(), "Wholesale".to_owned());
        data.insert("FKId".to_owned(), format!("{}", fkid));
        data.insert("FKFlag".to_owned(), format!("{}", fkflag));
        data.insert("CompanyName".to_owned(), name);
        data
    }
    /**
     * 解析提交内容
     */
    fn parse_submit_content(&self, res: Result<String, Error>) -> Result<i64, Error> {
        let x = match res {
            Ok(result) => {
                json::parse(result.as_str())},
            Err(_) => Err(json::Error::wrong_type(format!("{:?}", res).as_str())),
        };

        match x {
            Ok(obj) => {
                let mut order_id: i64 = 0;
                if obj["Success"] == true {
                    order_id = obj["Content"]["orderId"].as_i64().unwrap();
                    Ok(order_id)
                } else {
                    Err(err_info!(format!("{:?}", obj["Message"])))
                }
            }
            Err(e) => Err(err_info!(format!("{:?}", e))),
        }
    }
    /**
     * 发送订单回调
     */
    fn send_order_callback(&self, list: Vec<(u64, u32, i64)>) -> Result<Vec<i64>, Error> {
        let mut success_list: Vec<i64> = Vec::new();
        for t in list {
            let mut dic = HashMap::new();
            let order_id = t.2;
            let fk_id = t.0;
            dic.insert(String::from("OrderIds"), format!("{}", order_id));
            dic.insert(String::from("PayStatus"), String::from("true"));
            let res = self.post_data(&dic, "aus.package.order.callback");
            let content = self.parse_callback_content(res);
            match content {
                //更新内容
                Ok(s) => {
                    // let authorise = AppAuthorise::new(fk_id, t.1);
                    // authorise.update_aync_state();
                    println!("订单回调成功：fkid={:?},order_id={:?}",fk_id,order_id);
                    success_list.push(order_id);
                }
                Err(e) => {
                    println!(
                        "订单回调失败：fkid={:?},order_id={:?},{:?}",
                        fk_id,
                        order_id,
                        err_info!(format!("{:?}", e))
                    );
                }
            };
        }
        Ok(success_list)
    }
    /**
     * 解析回调内容
     */
    fn parse_callback_content(&self, res: Result<String, Error>) -> Result<i64, Error> {
        let x = match res {
            Ok(result) => json::parse(result.as_str()),
            Err(_) => Err(json::Error::wrong_type(format!("{:?}", res).as_str())),
        };

        match x {
            Ok(obj) => {
                if obj["Success"] == true {
                    Ok(1)
                } else {
                    Err(err_info!(format!("{:?}", obj["Content"])))
                }
            }
            Err(e) => {
                //println!("callback_content={:?}",x);
                Err(err_info!(format!("{:?}", e)))
            }
        }
    }
    /**
     * 发送数据
     */
    fn post_data(
        &self,
        data: &HashMap<String, String>,
        action: &str,
    ) -> Result<String, std::io::Error> {
        let mut dic = data.clone();
        dic.insert("SN".to_owned(), self.web.sn.to_owned());
        dic.insert("Method".to_owned(), action.to_owned());
        dic.insert("V".to_owned(), String::from("2.0"));
        use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
    
        let encode_token=utf8_percent_encode(&self.access_token.clone(), NON_ALPHANUMERIC).to_string();
        dic.insert(String::from("Token"), encode_token);
        dic.insert(
            String::from("Md5"),
            format!(
                "{:x}",
                md5::compute(format!("rwxkj:{}", self.access_token).as_bytes())
            ),
        );
        //println!("token={:?}",self.access_token);
        post(&self.web.api_domain, dic)
    }
}
