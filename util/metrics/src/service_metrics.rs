// Copyright (c) The Libra Core Contributors
// SPDX-License-Identifier: Apache-2.0
//
// Contains modifications by Botho.
// Copyright (c) 2024 Botho Foundation - tonic migration

/*!
`ServiceMetrics` is a metric [`Collector`](prometheus::core::Collector) to capture key
metrics about a gRPC server.

For each method, the counters that are captured are:
- num_req: number of requests
- num_error: number of errors (can be used to calculate error rate)
- num_status_code: number of gRPC status codes (to establish statistics on
  gRPC status codes, similar to how HTTP 2XX/4XX/5XX codes are profiled)
- duration: duration (in units determined by the exporter) the request took, bucketed

Example use:
call `req` when entering service method, and call `resp` on
exit, with a boolean flag to specify whether the request was
a success or a failure, to bump the counter for failures.
The call to `req` will provide a timer that handle time logging, as long
as it's in scope.

```ignore
async fn sample_service_method(request: Request<Params>) -> Result<Response<T>, Status> {
  let method_name = "MyService.sample_method".to_string();
  let _timer = metrics.req(&method_name);
  // do business logic
  metrics.resp(&method_name, success_flag);
}
```
*/

use bt_common::logger::global_log;
use prometheus::{
    core::{Collector, Desc},
    proto::MetricFamily,
    HistogramOpts, HistogramTimer, HistogramVec, IntCounterVec, Opts, Result,
};
use std::{path::Path, str};
use tonic::Code;

/// Helper that encapsulates boilerplate for tracking prometheus metrics about
/// gRPC services.
///
/// This struct defines several common metrics (with a distinct MetricFamily per
/// method) with the method path as a primary dimension/label. Method paths are
/// derived from request URI path. e.g., calc_service.req{method = "add"} = +1
/// e.g., calc_service.duration_sum{method="add"} = 6
///
/// Corresponds to which gRPC method is being called.
pub type GrpcMethodName = String;

#[derive(Clone)]
pub struct ServiceMetrics {
    /// Count of requests made by each gRPC method tracked
    num_req: IntCounterVec,

    /// Count of error responses for each gRPC method tracked
    num_error: IntCounterVec,

    /// Count of gRPC status codes for each gRPC method tracked
    num_status_code: IntCounterVec,

    /// Duration of gRPC method calls tracked
    duration: HistogramVec,
}
impl Default for ServiceMetrics {
    fn default() -> Self {
        let args = std::env::args().next().unwrap_or_default();
        let mut arg = Path::new(&args)
            .file_stem()
            .unwrap_or_default()
            .to_str()
            .unwrap_or_default()
            .replace('-', "_");
        if !arg.is_empty() {
            arg.push('_');
        }
        arg.push_str("grpc");
        ServiceMetrics::new_and_registered(arg)
    }
}

impl ServiceMetrics {
    /// Create a default constructor that initializes all metrics
    pub fn new<S: Into<String>>(name: S) -> ServiceMetrics {
        let name_str = name.into();

        ServiceMetrics {
            num_req: IntCounterVec::new(
                Opts::new(format!("{name_str}_num_req"), "Number of requests"),
                &["method"],
            )
            .unwrap(),
            num_error: IntCounterVec::new(
                Opts::new(format!("{name_str}_num_error"), "Number of errors"),
                &["method"],
            )
            .unwrap(),
            num_status_code: IntCounterVec::new(
                Opts::new(
                    format!("{name_str}_num_status_code"),
                    "Number of grpc status codes",
                ),
                &["method", "status_code"],
            )
            .unwrap(),
            duration: HistogramVec::new(
                //TODO: frumious: how to ensure units?
                HistogramOpts::new(
                    format!("{name_str}_duration"),
                    "Duration for a request, in units of time",
                ),
                &["method"],
            )
            .unwrap(),
        }
    }
}

impl ServiceMetrics {
    /// Register Prometheus metrics family
    pub fn new_and_registered<S: Into<String>>(name: S) -> ServiceMetrics {
        let svc = ServiceMetrics::new(name);
        let _res = prometheus::register(Box::new(svc.clone()));
        svc
    }

    /// Increments counters tracking the number of calls to a method
    /// and returns a timer to track the duration of the method.
    ///
    /// The method_name should be the gRPC method path like "service.Method".
    pub fn req(&self, method_name: &GrpcMethodName) -> Option<HistogramTimer> {
        self.num_req
            .with_label_values(&[method_name.as_str()])
            .inc();
        Some(
            self.duration
                .with_label_values(&[method_name.as_str()])
                .start_timer(),
        )
    }

    /// Increments an error counter if the method resulted in an error.
    pub fn resp(&self, method_name: &GrpcMethodName, success: bool) {
        self.num_error
            .with_label_values(&[method_name.as_str()])
            .inc_by(if success { 0 } else { 1 });
    }

    /// Increments a counter for the gRPC status code reported.
    pub fn status_code(&self, method_name: &GrpcMethodName, response_code: Code) {
        let code_str = format!("{:?}", response_code);
        self.num_status_code
            .with_label_values(&[method_name.as_str(), &code_str])
            .inc();
    }

    pub fn register_default(&self) -> Result<()> {
        prometheus::register(Box::new(self.clone()))
    }
}

impl Collector for ServiceMetrics {
    /// Collect metric descriptions for Prometheus
    fn desc(&self) -> Vec<&Desc> {
        // order: num_req, num_error, duration
        vec![
            self.num_req.desc(),
            self.num_error.desc(),
            self.num_status_code.desc(),
            self.duration.desc(),
        ]
        .into_iter()
        .map(|m| m[0])
        .collect()
    }

    /// Collect Prometheus metrics
    fn collect(&self) -> Vec<MetricFamily> {
        // families
        let vs = vec![
            self.num_req.collect(),
            self.num_error.collect(),
            self.num_status_code.collect(),
            self.duration.collect(),
        ];

        vs.into_iter().fold(vec![], |mut l, v| {
            l.extend(v);
            l
        })
    }
}

/// This method reads the full URI path which looks like
/// `/{package}.{service_name}/{method}` ('/' equates to ascii code 47)
/// and converts it into a dot-delimited string, dropping the 1st `/`
fn path_from_byte_slice(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 5 || bytes[0] != 47u8 {
        // Incorrect structure: too short, or first char is not '/'
        global_log::info!("malformed request path: {:?}", bytes);
        return None;
    }

    let mut method_raw = vec![0u8; bytes.len() - 1];
    method_raw.copy_from_slice(&bytes[1..]);
    if let Ok(name) = str::from_utf8(&method_raw) {
        return Some(name.replace('/', "."));
    }
    global_log::info!("failed to convert byte slice to string: {:?}", &method_raw);
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_from_bytes() {
        let too_short = vec![47u8, 65u8, 47u8];
        assert_eq!(path_from_byte_slice(&too_short), None);

        // first char is not '/'
        let malformed = vec![65u8, 46u8, 65u8, 47u8, 66u8];
        assert_eq!(path_from_byte_slice(&malformed), None);

        // /package.service/method
        let full_name = vec![47u8, 65u8, 46u8, 65u8, 47u8, 66u8];
        assert_eq!(
            path_from_byte_slice(&full_name),
            Some(String::from("A.A.B"))
        );

        // /service/method
        let no_package = vec![47u8, 65u8, 98u8, 47u8, 99u8];
        assert_eq!(
            path_from_byte_slice(&no_package),
            Some(String::from("Ab.c"))
        );
    }
}
