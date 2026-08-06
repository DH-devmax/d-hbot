//! 预测播报循环。
//!
//! 本文件是 `BackendRuntime` 的一部分；字段与私有辅助函数通过
//! `use super::*` 从 `runtime/mod.rs` 继承。

use super::*;

impl BackendRuntime {
    pub(super) async fn prediction_narration_loop(&self) {
        let mut receiver = self.prediction_narration_rx.lock().await;
        loop {
            let job = tokio::select! {
                _ = self.shutdown.cancelled() => break,
                job = receiver.recv() => match job {
                    Some(job) => job,
                    None => break,
                },
            };
            let work_id = runtime_lane_id("prediction", &job.account_id, job.group_id);
            let Ok(_permit) = self.ai_rule_gate.clone().acquire_owned().await else {
                self.coordination.tracker.start_named(
                    &work_id,
                    "prediction",
                    "生成预测说明",
                    "业务应用",
                    None,
                );
                self.coordination
                    .tracker
                    .finish(&work_id, "failed", "AI 调度器已停止");
                break;
            };
            self.coordination.tracker.start_named(
                &work_id,
                "prediction",
                "生成预测说明",
                "业务应用",
                None,
            );
            let outcome = match job.provider.decide(&job.request).await {
                Ok(decision)
                    if crate::business_apps::validate_prediction_narration(
                        &decision.reply,
                        &job.narration,
                    ) =>
                {
                    self.prediction_narration_cache.insert_scoped(
                        &job.account_id,
                        job.cache_key.clone(),
                        decision,
                    );
                    Ok(())
                }
                Ok(_) => Err("AI 预测说明格式不完整"),
                Err(_) => Err("AI 预测说明生成失败"),
            };
            match outcome {
                Ok(()) => self.coordination.tracker.finish(&work_id, "succeeded", ""),
                Err(error) => self.coordination.tracker.finish(&work_id, "failed", error),
            }
            self.prediction_narration_pending
                .lock()
                .await
                .remove(&job.cache_key);
        }
    }
}
