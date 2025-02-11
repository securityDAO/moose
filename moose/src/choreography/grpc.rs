//! gRPC-based choreography.

pub(crate) mod gen {
    tonic::include_proto!("moose_choreography");
}

use self::gen::choreography_server::{Choreography, ChoreographyServer};
use self::gen::{
    AbortComputationRequest, AbortComputationResponse, LaunchComputationRequest,
    LaunchComputationResponse, RetrieveResultsRequest, RetrieveResultsResponse,
};
use super::{NetworkingStrategy, StorageStrategy};
use crate::computation::{SessionId, Value};
use crate::execution::ExecutionContext;
use crate::execution::Identity;
use async_cell::sync::AsyncCell;
use async_trait::async_trait;
use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Serialize, Deserialize, PartialEq, Clone, Debug)]
pub struct ComputationOutputs {
    pub outputs: HashMap<String, Value>,
    pub elapsed_time: Option<Duration>,
}

type ResultStores = DashMap<SessionId, Arc<AsyncCell<ComputationOutputs>>>;

pub struct GrpcChoreography {
    own_identity: Identity,
    choreographer: Option<String>,
    result_stores: Arc<ResultStores>,
    networking_strategy: NetworkingStrategy,
    storage_strategy: StorageStrategy,
}

impl GrpcChoreography {
    pub fn new(
        own_identity: Identity,
        choreographer: Option<String>,
        networking_strategy: NetworkingStrategy,
        storage_strategy: StorageStrategy,
    ) -> GrpcChoreography {
        GrpcChoreography {
            own_identity,
            choreographer,
            result_stores: Arc::new(ResultStores::default()),
            networking_strategy,
            storage_strategy,
        }
    }

    pub fn into_server(self) -> ChoreographyServer<impl Choreography> {
        ChoreographyServer::new(self)
    }
}

impl GrpcChoreography {
    fn check_choreographer<T>(&self, request: &tonic::Request<T>) -> Result<(), tonic::Status> {
        let choreographer = crate::grpc::extract_sender(request).map_err(|_e| {
            tonic::Status::new(
                tonic::Code::Aborted,
                "failed to extract sender identity".to_string(),
            )
        })?;

        match (&self.choreographer, choreographer) {
            (None, None) => Ok(()),
            (None, Some(_actual)) => Err(tonic::Status::new(
                tonic::Code::Aborted,
                "did not expect choreographer certificate".to_string(),
            )),
            (Some(_expected), None) => Err(tonic::Status::new(
                tonic::Code::Aborted,
                "expected choreographer certificate".to_string(),
            )),
            (Some(expected), Some(actual)) => {
                if expected != &actual {
                    Err(tonic::Status::new(
                        tonic::Code::Aborted,
                        "expected choreographer did not match actual".to_string(),
                    ))
                } else {
                    Ok(())
                }
            }
        }
    }
}

#[async_trait]
impl Choreography for GrpcChoreography {
    async fn launch_computation(
        &self,
        request: tonic::Request<LaunchComputationRequest>,
    ) -> Result<tonic::Response<LaunchComputationResponse>, tonic::Status> {
        tracing::info!("Launching computation");

        self.check_choreographer(&request)?;
        let request = request.into_inner();

        let session_id = bincode::deserialize::<SessionId>(&request.session_id).map_err(|_e| {
            tonic::Status::new(
                tonic::Code::Aborted,
                "failed to parse session id".to_string(),
            )
        })?;

        match self.result_stores.entry(session_id.clone()) {
            Entry::Occupied(_) => Err(tonic::Status::new(
                tonic::Code::Aborted,
                "session id exists already or inconsistent metric and result map".to_string(),
            )),
            Entry::Vacant(result_stores_entry) => {
                let result_cell = AsyncCell::shared();
                result_stores_entry.insert(result_cell);

                let computation = bincode::deserialize(&request.computation).map_err(|_e| {
                    tonic::Status::new(
                        tonic::Code::Aborted,
                        "failed to parse computation".to_string(),
                    )
                })?;

                let arguments = bincode::deserialize(&request.arguments).map_err(|_e| {
                    tonic::Status::new(
                        tonic::Code::Aborted,
                        "failed to parse arguments".to_string(),
                    )
                })?;

                let role_assignments =
                    bincode::deserialize(&request.role_assignment).map_err(|_e| {
                        tonic::Status::new(
                            tonic::Code::Aborted,
                            "failed to parse role assignment".to_string(),
                        )
                    })?;

                let own_identity = self.own_identity.clone();
                let networking = (self.networking_strategy)(session_id.clone());
                let storage = (self.storage_strategy)();
                let context = ExecutionContext::new(own_identity, networking, storage);

                let execution_start_timer = Instant::now();

                let (_handle, outputs) = context
                    .execute_computation(
                        session_id.clone(),
                        &computation,
                        arguments,
                        role_assignments,
                    )
                    .await
                    .map_err(|_e| {
                        tonic::Status::new(
                            tonic::Code::Aborted,
                            "failed launch computation".to_string(),
                        )
                    })?;

                let result_stores = Arc::clone(&self.result_stores);

                tokio::spawn(async move {
                    let mut results = HashMap::with_capacity(outputs.len());
                    for (output_name, output_value) in outputs {
                        let value = output_value.await.unwrap();
                        results.insert(output_name, value);
                    }
                    tracing::info!("Results ready, {:?}", results.keys());

                    let result_cell = result_stores
                        .get(&session_id)
                        .expect("session disappeared unexpectedly");

                    let execution_stop_timer = Instant::now();
                    let elapsed_time = execution_stop_timer.duration_since(execution_start_timer);
                    result_cell.set(ComputationOutputs {
                        outputs: results,
                        elapsed_time: Some(elapsed_time),
                    });
                });

                Ok(tonic::Response::new(LaunchComputationResponse::default()))
            }
        }
    }

    async fn abort_computation(
        &self,
        _request: tonic::Request<AbortComputationRequest>,
    ) -> Result<tonic::Response<AbortComputationResponse>, tonic::Status> {
        unimplemented!()
    }

    async fn retrieve_results(
        &self,
        request: tonic::Request<RetrieveResultsRequest>,
    ) -> Result<tonic::Response<RetrieveResultsResponse>, tonic::Status> {
        self.check_choreographer(&request)?;
        let request = request.into_inner();

        let session_id = bincode::deserialize::<SessionId>(&request.session_id).map_err(|_e| {
            tonic::Status::new(
                tonic::Code::Aborted,
                "failed to parse session id".to_string(),
            )
        })?;

        match self.result_stores.get(&session_id) {
            Some(results) => {
                let results = results.value().get().await;
                let values = bincode::serialize(&results).expect("failed to serialize results");

                Ok(tonic::Response::new(RetrieveResultsResponse { values }))
            }
            None => Err(tonic::Status::new(
                tonic::Code::NotFound,
                "unknown session id".to_string(),
            )),
        }
    }
}
