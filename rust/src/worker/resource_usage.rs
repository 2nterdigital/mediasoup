use crate::messages::Request;
use mediasoup_sys::fbs::{message, request, response, worker};
use planus::Builder;
use serde::{Deserialize, Serialize};
use std::error::Error;

/// Resource usage returned by the native mediasoup worker.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, Eq, PartialEq)]
#[non_exhaustive]
pub struct WorkerResourceUsage {
    /// User CPU time used, in milliseconds.
    pub ru_utime: u64,
    /// System CPU time used, in milliseconds.
    pub ru_stime: u64,
    /// Maximum resident set size.
    pub ru_maxrss: u64,
    /// Integral shared memory size.
    pub ru_ixrss: u64,
    /// Integral unshared data size.
    pub ru_idrss: u64,
    /// Integral unshared stack size.
    pub ru_isrss: u64,
    /// Page reclaims (soft page faults).
    pub ru_minflt: u64,
    /// Page faults (hard page faults).
    pub ru_majflt: u64,
    /// Swaps.
    pub ru_nswap: u64,
    /// Block input operations.
    pub ru_inblock: u64,
    /// Block output operations.
    pub ru_oublock: u64,
    /// IPC messages sent.
    pub ru_msgsnd: u64,
    /// IPC messages received.
    pub ru_msgrcv: u64,
    /// Signals received.
    pub ru_nsignals: u64,
    /// Voluntary context switches.
    pub ru_nvcsw: u64,
    /// Involuntary context switches.
    pub ru_nivcsw: u64,
}

#[derive(Debug)]
pub(super) struct WorkerGetResourceUsageRequest {}

fn map_worker_resource_usage(data: worker::ResourceUsageResponse) -> WorkerResourceUsage {
    WorkerResourceUsage {
        ru_utime: data.ru_utime,
        ru_stime: data.ru_stime,
        ru_maxrss: data.ru_maxrss,
        ru_ixrss: data.ru_ixrss,
        ru_idrss: data.ru_idrss,
        ru_isrss: data.ru_isrss,
        ru_minflt: data.ru_minflt,
        ru_majflt: data.ru_majflt,
        ru_nswap: data.ru_nswap,
        ru_inblock: data.ru_inblock,
        ru_oublock: data.ru_oublock,
        ru_msgsnd: data.ru_msgsnd,
        ru_msgrcv: data.ru_msgrcv,
        ru_nsignals: data.ru_nsignals,
        ru_nvcsw: data.ru_nvcsw,
        ru_nivcsw: data.ru_nivcsw,
    }
}

impl Request for WorkerGetResourceUsageRequest {
    const METHOD: request::Method = request::Method::WorkerGetResourceUsage;
    type HandlerId = &'static str;
    type Response = WorkerResourceUsage;

    fn into_bytes(self, id: u32, handler_id: Self::HandlerId) -> Vec<u8> {
        let mut builder = Builder::new();

        let request = request::Request::create(
            &mut builder,
            id,
            Self::METHOD,
            handler_id.to_string(),
            None::<request::Body>,
        );
        let message_body = message::Body::create_request(&mut builder, request);
        let message = message::Message::create(&mut builder, message_body);

        builder.finish(message, None).to_vec()
    }

    fn convert_response(
        response: Option<response::BodyRef<'_>>,
    ) -> Result<Self::Response, Box<dyn Error + Send + Sync>> {
        let Some(response::BodyRef::WorkerResourceUsageResponse(data)) = response else {
            panic!("Wrong message from worker: {response:?}");
        };

        let data = worker::ResourceUsageResponse::try_from(data)?;

        Ok(map_worker_resource_usage(data))
    }
}

#[cfg(test)]
mod tests {
    use super::{map_worker_resource_usage, WorkerResourceUsage};
    use mediasoup_sys::fbs::worker;

    #[test]
    fn worker_resource_usage_maps_every_native_protocol_field() {
        let mapped = map_worker_resource_usage(worker::ResourceUsageResponse {
            ru_utime: 1,
            ru_stime: 2,
            ru_maxrss: 3,
            ru_ixrss: 4,
            ru_idrss: 5,
            ru_isrss: 6,
            ru_minflt: 7,
            ru_majflt: 8,
            ru_nswap: 9,
            ru_inblock: 10,
            ru_oublock: 11,
            ru_msgsnd: 12,
            ru_msgrcv: 13,
            ru_nsignals: 14,
            ru_nvcsw: 15,
            ru_nivcsw: 16,
        });

        assert_eq!(
            mapped,
            WorkerResourceUsage {
                ru_utime: 1,
                ru_stime: 2,
                ru_maxrss: 3,
                ru_ixrss: 4,
                ru_idrss: 5,
                ru_isrss: 6,
                ru_minflt: 7,
                ru_majflt: 8,
                ru_nswap: 9,
                ru_inblock: 10,
                ru_oublock: 11,
                ru_msgsnd: 12,
                ru_msgrcv: 13,
                ru_nsignals: 14,
                ru_nvcsw: 15,
                ru_nivcsw: 16,
            }
        );
    }
}
