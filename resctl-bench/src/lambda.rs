// Copyright (c) Facebook, Inc. and its affiliates.
// Copyright (c) Collabora Ltd.
use anyhow::{anyhow, Context, Error, Result};
use aws_sdk_s3::{
    error::{GetObjectError, GetObjectErrorKind},
    SdkError,
};
use log::error;
use std::io::{Cursor, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;

use rd_util::{LambdaRequest as Request, LambdaResponse as Response};

use crate::job::{FormatOpts, JobCtxs};

// The hard-coded file name is safe because the lambda function runs single-threaded
// and isolated - each concurrent instance runs on its own environment.
const RESULT_PATH: &'static str = "/tmp/result.json.gz";

pub fn init_lambda() {
    let executable_path = std::env::current_exe().expect("Failed to get executable path");

    let executable_name = executable_path
        .file_name()
        .expect("Failed to get executable name")
        .to_string_lossy()
        .to_string();

    if executable_name == "bootstrap" && std::env::args().len() == 1 {
        let error = std::process::Command::new(executable_path)
            .args(&["--result", RESULT_PATH, "lambda"])
            .exec();
        error!("Failed to re-exec the process for lambda ({:#})", &error);
        panic!();
    }
}

#[tokio::main]
pub async fn run() -> Result<()> {
    let handler = |request: Request, _| async move {
        let helper = LambdaHelper::new().await;

        // Unpack the base64 encoded gz-compressed file. This is safe because Lambda has a hard
        // limit on the size of the requests (6MB at the moment).
        let data = base64::decode(&request.data)?;

        // Loading the results and formatting sysinfo and summary serve as validation
        // that the uploaded file is a properly formated benchmark result.
        let jctxs = helper.load_results(&data).await?;

        let sysinfo = helper.format_sysinfo(&jctxs)?;
        let summary = helper.format_summary(&jctxs)?;

        // Valid! Let's check we do not have a duplicate and upload to S3.
        let object_name = helper.object_name_from_hash(&data)?;

        if helper.s3_object_exists(&object_name).await? {
            return Ok(Response {
                issue: None,
                error_type: Some(format!("Custom")),
                error_message: Some(format!("This file has already been submitted.")),
            });
        }

        let s3_url = helper.save_to_s3(&object_name).await?;

        // Now we just need to tell the world.
        let identification = helper.format_submitter_info(&request);
        let issue_url = helper
            .create_github_issue(
                &sysinfo,
                &format!("{}\n\n{}```\n{}\n```", s3_url, identification, summary),
            )
            .await?;

        Ok::<_, Error>(Response {
            issue: Some(issue_url),
            error_type: None,
            error_message: None,
        })
    };

    lambda_runtime::run(lambda_runtime::handler_fn(handler))
        .await
        .map_err(|e| anyhow!(e))?;
    Ok(())
}

pub struct LambdaHelper {
    s3: aws_sdk_s3::Client,
    ssm: aws_sdk_ssm::Client,
}

impl LambdaHelper {
    pub async fn new() -> Self {
        let aws_config = aws_config::load_from_env().await;

        LambdaHelper {
            s3: aws_sdk_s3::Client::new(&aws_config),
            ssm: aws_sdk_ssm::Client::new(&aws_config),
        }
    }

    pub async fn load_results(&self, data: &[u8]) -> Result<JobCtxs> {
        // Write the actual data to the result file so we can have JobCtxs load it.
        let mut file = std::fs::File::create(RESULT_PATH)?;
        file.write_all(&data)?;

        JobCtxs::load_results(RESULT_PATH)
    }

    pub async fn s3_object_exists(&self, object_name: &str) -> Result<bool> {
        let output = self
            .s3
            .get_object()
            .bucket("iocost-submit")
            .key(object_name)
            .send()
            .await;

        match output {
            Ok(_) => Ok(true),
            Err(SdkError::ServiceError {
                err:
                    GetObjectError {
                        kind: GetObjectErrorKind::NoSuchKey(..),
                        ..
                    },
                ..
            }) => Ok(false),
            Err(e) => Err(anyhow!(e)),
        }
    }

    pub async fn save_to_s3(&self, object_name: &str) -> Result<String> {
        let body = aws_sdk_s3::ByteStream::from_path(Path::new(RESULT_PATH)).await?;
        self.s3
            .put_object()
            .bucket("iocost-submit")
            .key(object_name)
            .body(body)
            .send()
            .await?;

        Ok(format!(
            "https://iocost-submit.s3.eu-west-1.amazonaws.com/{}",
            object_name
        ))
    }

    pub async fn create_github_issue(&self, title: &str, body: &str) -> Result<String> {
        let token = self
            .ssm
            .get_parameter()
            .set_name(Some("/iocost-bot/token".to_string()))
            .send()
            .await
            .expect("Failed to query parameter")
            .parameter
            .expect("Could not find parameter")
            .value
            .expect("Parameter value is None");

        let issue = octocrab::OctocrabBuilder::new()
            .personal_token(token)
            .build()?
            .issues("iocost-benchmark", "iocost-benchmarks")
            .create(title)
            .body(body)
            .send()
            .await?;

        Ok(issue.html_url.to_string())
    }

    pub fn object_name_from_hash(&self, data: &[u8]) -> Result<String> {
        // Use the actual content for the hash to avoid adding duplicates just because
        // of differences in the compression.
        let mut uncompressed_data = Vec::<u8>::new();
        libflate::gzip::Decoder::new(Cursor::new(&data))
            .context("Creating gzip decoder")?
            .read_to_end(&mut uncompressed_data)
            .context("Decompressing")?;

        Ok(format!(
            "result-{:x}.json.gz",
            md5::compute(uncompressed_data)
        ))
    }

    pub fn format_sysinfo(&self, jctxs: &JobCtxs) -> Result<String> {
        let sysinfo = &jctxs
            .vec
            .iter()
            .find(|job| job.data.sysinfo.sysreqs_report.is_some())
            .ok_or_else(|| anyhow!("No sysinfo found on job"))?
            .data
            .sysinfo;
        let sysrep = sysinfo.sysreqs_report.as_ref().unwrap();

        Ok(format!(
            "{} (fwrev: {}) | bench version {}",
            sysrep.scr_dev_model, sysrep.scr_dev_fwrev, sysinfo.bench_version
        ))
    }

    pub fn format_summary(&self, jctxs: &JobCtxs) -> Result<String> {
        let format_opts = FormatOpts {
            full: false,
            merge_only: false,
            rstat: 0,
            result_path: RESULT_PATH,
        };
        let empty_props = vec![Default::default()];

        let mut summary = String::new();
        for job in jctxs.vec.iter() {
            if !job.data.result.is_some() {
                continue;
            }

            summary.push_str(&format!(
                "{}\n\n{}\n",
                "=".repeat(90),
                &job.format(&format_opts, &empty_props)?
            ));
        }

        Ok(summary)
    }

    pub fn format_submitter_info(&self, request: &Request) -> String {
        let mut id_str = String::new();

        if let Some(email) = &request.email {
            id_str.push_str("Submitter email: ");
            id_str.push_str(email);
            id_str.push_str("\n");
        }

        if let Some(github) = &request.github {
            id_str.push_str("Submitter github user: ");
            if !github.starts_with('@') {
                id_str.push('@');
            }
            id_str.push_str(github);
            id_str.push_str("\n");
        }

        // Add some spacing around what we have.
        if !id_str.is_empty() {
            return format!("\n\n{}\n\n", id_str);
        }

        id_str
    }
}

impl Drop for LambdaHelper {
    fn drop(&mut self) {
        std::fs::remove_file(RESULT_PATH).ok();
    }
}
