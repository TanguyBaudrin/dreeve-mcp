//! The MCP server: every tool maps 1:1 onto a Dreeve HTTP API operation.

use http::request::Parts;
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{tool::Extension, wrapper::Parameters},
    model::{CallToolResult, ContentBlock},
    schemars, tool, tool_handler, tool_router,
};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::api::DreeveApi;

/// Tools-only MCP server backed by the Dreeve API.
#[derive(Clone)]
pub struct DreeveServer {
    api: DreeveApi,
}

impl DreeveServer {
    pub fn new(api: DreeveApi) -> Self {
        Self { api }
    }

    /// The `Authorization` header of the MCP request calling the tool,
    /// forwarded verbatim to the Dreeve API. The HTTP layer already answers
    /// `401` to key-less `POST`s; this is the tool-level safety net.
    fn authorization(parts: &Parts) -> Result<&str, McpError> {
        parts
            .headers
            .get(http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                McpError::invalid_params(
                    "Missing Authorization header: the MCP client must send the \
                     Dreeve API key as `Authorization: Bearer drv_…`.",
                    None,
                )
            })
    }
}

// ---------------------------------------------------------------------------
// Result helpers
// ---------------------------------------------------------------------------

/// Success result carrying pretty-printed JSON.
fn ok_json(value: &Value) -> Result<CallToolResult, McpError> {
    let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

/// Success result carrying raw text (used for GPX documents).
fn ok_text(text: &str) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

/// Tool-level error: the caller sees the message (upstream 4xx/5xx, bad file…).
fn fail(message: impl std::fmt::Display) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::error(vec![ContentBlock::text(
        message.to_string(),
    )]))
}

// ---------------------------------------------------------------------------
// Parameter types
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListActivitiesParams {
    /// Only activities that started on this day or later, as `YYYY-MM-DD`.
    #[serde(default)]
    pub from: Option<String>,
    /// Only activities that started on this day or earlier, as `YYYY-MM-DD`.
    #[serde(default)]
    pub to: Option<String>,
    /// One sport type, or several separated by commas: `Ride` or `Run,TrailRun,Hike`.
    #[serde(default)]
    pub sport_type: Option<String>,
    /// Only activities that have (`true`) or do not have (`false`) a GPX track.
    #[serde(default)]
    pub has_gpx: Option<bool>,
    /// Page number, starting at 1.
    #[serde(default)]
    pub page: Option<u32>,
    /// Page size, 1 to 100 (API default: 25).
    #[serde(default)]
    pub size: Option<u32>,
}

fn list_activities_query(p: &ListActivitiesParams) -> Vec<(&'static str, String)> {
    let mut q: Vec<(&str, String)> = Vec::new();
    if let Some(v) = &p.from {
        q.push(("filters[from]", v.clone()));
    }
    if let Some(v) = &p.to {
        q.push(("filters[to]", v.clone()));
    }
    if let Some(v) = &p.sport_type {
        q.push(("filters[sportType]", v.clone()));
    }
    if let Some(v) = p.has_gpx {
        q.push(("filters[hasGpx]", v.to_string()));
    }
    if let Some(v) = p.page {
        q.push(("pagination[page]", v.to_string()));
    }
    if let Some(v) = p.size {
        q.push(("pagination[size]", v.to_string()));
    }
    q
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActivityIdParams {
    /// The prefixed activity id, e.g. `activity-9756441741`.
    pub activity_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateActivityParams {
    /// The prefixed activity id, e.g. `activity-9756441741`.
    pub activity_id: String,
    /// New name for the activity.
    #[serde(default)]
    pub name: Option<String>,
    /// New sport type using Strava's vocabulary: `Ride`, `GravelRide`, `TrailRun`, `Hike`…
    #[serde(default)]
    pub sport_type: Option<String>,
    /// New description. Send an empty string to clear the description.
    #[serde(default)]
    pub description: Option<String>,
    /// Existing gear id to attach, e.g. `gear-b12659861`. Send an empty string to remove the gear.
    #[serde(default)]
    pub gear_id: Option<String>,
}

/// Build the PATCH body; rejects an empty update as the API would (400).
fn update_activity_body(p: &UpdateActivityParams) -> Result<Value, String> {
    let mut body = Map::new();
    if let Some(v) = &p.name {
        if v.is_empty() {
            return Err(r#""name" must not be empty."#.into());
        }
        body.insert("name".into(), json!(v));
    }
    if let Some(v) = &p.sport_type {
        if v.is_empty() {
            return Err(r#""sportType" must not be empty."#.into());
        }
        body.insert("sportType".into(), json!(v));
    }
    if let Some(v) = &p.description {
        body.insert(
            "description".into(),
            if v.is_empty() { Value::Null } else { json!(v) },
        );
    }
    if let Some(v) = &p.gear_id {
        body.insert(
            "gearId".into(),
            if v.is_empty() { Value::Null } else { json!(v) },
        );
    }
    if body.is_empty() {
        return Err("Provide at least one of: name, sportType, description, gearId.".to_string());
    }
    Ok(Value::Object(body))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActivityImageIdParams {
    /// The prefixed activity id, e.g. `activity-9756441741`.
    pub activity_id: String,
    /// The prefixed image id as listed in the activity's `images`, e.g.
    /// `activityImage-0a8cdfbb-63b4-4f0f-a09d-faedd5a00753`.
    pub image_id: String,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListFileImportsParams {
    /// Only imports of the file with exactly this name, e.g. `morning-ride.fit`.
    #[serde(default)]
    pub filename: Option<String>,
    /// One status, or several separated by commas: `queued`, `success`, `skipped`, `failed`.
    #[serde(default)]
    pub status: Option<String>,
    /// One source, or several separated by commas: `fitFile`, `tcxFile`, `gpxFile`.
    #[serde(default)]
    pub source: Option<String>,
    /// Page number, starting at 1.
    #[serde(default)]
    pub page: Option<u32>,
    /// Page size, 1 to 100 (API default: 25).
    #[serde(default)]
    pub size: Option<u32>,
}

fn list_file_imports_query(p: &ListFileImportsParams) -> Vec<(&'static str, String)> {
    let mut q: Vec<(&str, String)> = Vec::new();
    if let Some(v) = &p.filename {
        q.push(("filters[filename]", v.clone()));
    }
    if let Some(v) = &p.status {
        q.push(("filters[status]", v.clone()));
    }
    if let Some(v) = &p.source {
        q.push(("filters[source]", v.clone()));
    }
    if let Some(v) = p.page {
        q.push(("pagination[page]", v.to_string()));
    }
    if let Some(v) = p.size {
        q.push(("pagination[size]", v.to_string()));
    }
    q
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UploadActivityFileParams {
    /// Absolute path of a `.fit`, `.tcx` or `.gpx` file **on the machine running
    /// dreeve-mcp** (mount the directory into the container when self-hosting).
    pub path: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AddActivityImageParams {
    /// The prefixed activity id, e.g. `activity-9756441741`.
    pub activity_id: String,
    /// Absolute path of a jpg, png or webp image on the machine running dreeve-mcp.
    pub path: String,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListMaintenanceTasksParams {
    /// Only tasks of components currently fitted on this gear, e.g. `gear-b12659861`.
    #[serde(default)]
    pub gear_id: Option<String>,
}

fn list_maintenance_tasks_query(p: &ListMaintenanceTasksParams) -> Vec<(&'static str, String)> {
    match &p.gear_id {
        Some(v) => vec![("filters[gearId]", v.clone())],
        None => Vec::new(),
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RecordWeightParams {
    /// The measurement in the instance's unit system: kilograms for Metric, pounds for Imperial.
    pub weight: f64,
    /// The measurement date as `YYYY-MM-DD`.
    pub on: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeleteWeightParams {
    /// The measurement date as `YYYY-MM-DD`.
    pub on: String,
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

#[tool_router]
impl DreeveServer {
    #[tool(
        description = "Check the connection to the Dreeve instance. Returns the app name, the running version and whether this instance accepts file uploads (false when IMPORT_MODE is not `files`)."
    )]
    async fn check_status(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, McpError> {
        let auth = Self::authorization(&parts)?;
        match self.api.status(auth).await {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        }
    }

    #[tool(
        description = "Search activities, newest first. All filters are optional: date range (YYYY-MM-DD), one or more comma-separated sport types, presence of a GPX track, and pagination (page >= 1, size 1..100)."
    )]
    async fn list_activities(
        &self,
        Parameters(params): Parameters<ListActivitiesParams>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, McpError> {
        let auth = Self::authorization(&parts)?;
        match self
            .api
            .get("/api/v1/activities", &list_activities_query(&params), auth)
            .await
        {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        }
    }

    #[tool(
        description = "Get one activity in full detail (stats, heart rate, power, location, images, gear…) by its prefixed id."
    )]
    async fn get_activity(
        &self,
        Parameters(params): Parameters<ActivityIdParams>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, McpError> {
        let auth = Self::authorization(&parts)?;
        match self
            .api
            .get(
                &format!("/api/v1/activities/{}", params.activity_id),
                &[],
                auth,
            )
            .await
        {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        }
    }

    #[tool(
        description = "Update an activity: change its name, sport type, description or gear. Provide at least one field; an empty description or gearId clears that field. Only works when the instance runs in file import mode."
    )]
    async fn update_activity(
        &self,
        Parameters(params): Parameters<UpdateActivityParams>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, McpError> {
        let auth = Self::authorization(&parts)?;
        let body = match update_activity_body(&params) {
            Ok(b) => b,
            Err(e) => return fail(e),
        };
        match self
            .api
            .patch(
                &format!("/api/v1/activities/{}", params.activity_id),
                &body,
                auth,
            )
            .await
        {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        }
    }

    #[tool(
        description = "Delete an activity together with its streams, laps, splits, best efforts and import log. This is irreversible — only call it when the user explicitly asks for deletion. Only works when the instance runs in file import mode."
    )]
    async fn delete_activity(
        &self,
        Parameters(params): Parameters<ActivityIdParams>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, McpError> {
        let auth = Self::authorization(&parts)?;
        match self
            .api
            .delete(&format!("/api/v1/activities/{}", params.activity_id), auth)
            .await
        {
            Ok(()) => ok_text("Activity deleted."),
            Err(e) => fail(e),
        }
    }

    #[tool(
        description = "Download an activity route as a GPX 1.1 document (raw XML text). Only activities with `hasGpx: true` can be exported — check that field first, manual and indoor activities have no GPS data."
    )]
    async fn download_activity_gpx(
        &self,
        Parameters(params): Parameters<ActivityIdParams>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, McpError> {
        let auth = Self::authorization(&parts)?;
        match self
            .api
            .get_text(
                &format!("/api/v1/activities/{}/gpx", params.activity_id),
                auth,
            )
            .await
        {
            Ok(gpx) => ok_text(&gpx),
            Err(e) => fail(e),
        }
    }

    #[tool(
        description = "Add a photo (jpg, png or webp) to an activity. Uploads the file at `path` from the machine running dreeve-mcp. The upload is asynchronous on the Dreeve side; only works when the instance runs in file import mode."
    )]
    async fn add_activity_image(
        &self,
        Parameters(params): Parameters<AddActivityImageParams>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, McpError> {
        let auth = Self::authorization(&parts)?;
        match self
            .api
            .post_file(
                &format!("/api/v1/activities/{}/images", params.activity_id),
                &params.path,
                auth,
            )
            .await
        {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        }
    }

    #[tool(
        description = "Remove an image from an activity and delete the file. `imageId` is the prefixed id from the activity's `images` list. Only works when the instance runs in file import mode."
    )]
    async fn remove_activity_image(
        &self,
        Parameters(params): Parameters<ActivityImageIdParams>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, McpError> {
        let auth = Self::authorization(&parts)?;
        match self
            .api
            .delete(
                &format!(
                    "/api/v1/activities/{}/images/{}",
                    params.activity_id, params.image_id
                ),
                auth,
            )
            .await
        {
            Ok(()) => ok_text("Image removed."),
            Err(e) => fail(e),
        }
    }

    #[tool(
        description = "Upload a .fit, .tcx or .gpx activity file from the machine running dreeve-mcp (mount the directory into the container). Returns 202: the import is queued — poll list_file_imports afterwards to see the outcome (success/skipped/failed). Fails with 409 when the instance does not run in file import mode."
    )]
    async fn upload_activity_file(
        &self,
        Parameters(params): Parameters<UploadActivityFileParams>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, McpError> {
        let auth = Self::authorization(&parts)?;
        match self
            .api
            .post_file("/api/v1/activity/upload", &params.path, auth)
            .await
        {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        }
    }

    #[tool(
        description = "Search file imports, queued files first then newest first. Each entry shows filename, source (fitFile/tcxFile/gpxFile), status (queued/success/skipped/failed), errorMessage and the resulting activityId. Filters: exact filename, comma-separated statuses, comma-separated sources; plus pagination."
    )]
    async fn list_file_imports(
        &self,
        Parameters(params): Parameters<ListFileImportsParams>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, McpError> {
        let auth = Self::authorization(&parts)?;
        match self
            .api
            .get(
                "/api/v1/file-imports",
                &list_file_imports_query(&params),
                auth,
            )
            .await
        {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        }
    }

    #[tool(
        description = "List all gear, active gear first: id, name, type (imported/custom), retired flag, total distance and number of activities."
    )]
    async fn list_gear(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, McpError> {
        let auth = Self::authorization(&parts)?;
        match self.api.get("/api/v1/gear", &[], auth).await {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        }
    }

    #[tool(
        description = "List gear maintenance tasks, most urgent first: label, component, interval, elapsed/remaining/percentage and isDue/isOverdue flags. Optionally filter by gear id to only see tasks of components fitted on that gear."
    )]
    async fn list_maintenance_tasks(
        &self,
        Parameters(params): Parameters<ListMaintenanceTasksParams>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, McpError> {
        let auth = Self::authorization(&parts)?;
        match self
            .api
            .get(
                "/api/v1/gear/maintenance-tasks",
                &list_maintenance_tasks_query(&params),
                auth,
            )
            .await
        {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        }
    }

    #[tool(
        description = "List weight measurements, newest to oldest, in the instance's configured unit system (kilograms for Metric, pounds for Imperial)."
    )]
    async fn list_weights(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, McpError> {
        let auth = Self::authorization(&parts)?;
        match self.api.get("/api/v1/athlete/weights", &[], auth).await {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        }
    }

    #[tool(
        description = "Record a weight measurement for a calendar date, or replace the existing measurement for that date. `weight` must be > 0, in the instance's unit system (kg for Metric, lb for Imperial); `on` is YYYY-MM-DD."
    )]
    async fn record_weight(
        &self,
        Parameters(params): Parameters<RecordWeightParams>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, McpError> {
        let auth = Self::authorization(&parts)?;
        if params.weight <= 0.0 {
            return fail(r#""weight" must be greater than 0."#);
        }
        let body = json!({ "weight": params.weight, "on": params.on });
        match self
            .api
            .post_json("/api/v1/athlete/weights", &body, auth)
            .await
        {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        }
    }

    #[tool(
        description = "Delete the weight measurement of a calendar date (YYYY-MM-DD). Does nothing if no measurement exists for that date."
    )]
    async fn delete_weight(
        &self,
        Parameters(params): Parameters<DeleteWeightParams>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, McpError> {
        let auth = Self::authorization(&parts)?;
        match self
            .api
            .delete(&format!("/api/v1/athlete/weights/{}", params.on), auth)
            .await
        {
            Ok(()) => ok_text("Weight measurement deleted."),
            Err(e) => fail(e),
        }
    }
}

#[tool_handler(
    name = "dreeve-mcp",
    instructions = "Tools talk to a self-hosted Dreeve instance; the base URL is configured in the server's environment (DREEVE_API_URL) and the API key is sent per request by the MCP client in the `Authorization: Bearer …` header (from Dreeve → Settings → Security → API access). Ids are prefixed: activities look like `activity-9756441741`, gear like `gear-b12659861`, images like `activityImage-…` — use ids returned by the list/get tools verbatim. Dates are `YYYY-MM-DD`, weights follow the instance's unit system (Metric = kg, Imperial = lb), and sport types use Strava's vocabulary (Ride, GravelRide, Run, TrailRun, Hike…). Uploads (upload_activity_file, add_activity_image) read a file path from the machine running dreeve-mcp, not inline data, and their imports are asynchronous: poll list_file_imports for the outcome. Update and delete operations require the instance to run with IMPORT_MODE=files."
)]
impl ServerHandler for DreeveServer {}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts_with_auth(value: Option<&str>) -> Parts {
        let mut builder = http::Request::builder().uri("/mcp");
        if let Some(value) = value {
            builder = builder.header(http::header::AUTHORIZATION, value);
        }
        builder.body(()).expect("request").into_parts().0
    }

    #[test]
    fn authorization_reads_the_bearer_header_verbatim() {
        let parts = parts_with_auth(Some("Bearer drv_abc123"));
        assert_eq!(
            DreeveServer::authorization(&parts).unwrap(),
            "Bearer drv_abc123"
        );
    }

    #[test]
    fn authorization_rejects_a_missing_header() {
        let parts = parts_with_auth(None);
        let err = DreeveServer::authorization(&parts).unwrap_err();
        assert!(err.message.contains("Authorization"), "{}", err.message);
    }

    #[test]
    fn authorization_rejects_an_empty_header() {
        let parts = parts_with_auth(Some("   "));
        assert!(DreeveServer::authorization(&parts).is_err());
    }

    fn update_params() -> UpdateActivityParams {
        UpdateActivityParams {
            activity_id: "activity-1".into(),
            name: None,
            sport_type: None,
            description: None,
            gear_id: None,
        }
    }

    #[test]
    fn list_activities_query_only_includes_set_filters() {
        let q = list_activities_query(&ListActivitiesParams::default());
        assert!(q.is_empty());

        let q = list_activities_query(&ListActivitiesParams {
            from: Some("2023-09-04".into()),
            sport_type: Some("Ride,TrailRun".into()),
            has_gpx: Some(true),
            size: Some(50),
            ..Default::default()
        });
        assert_eq!(
            q,
            vec![
                ("filters[from]", "2023-09-04".to_string()),
                ("filters[sportType]", "Ride,TrailRun".to_string()),
                ("filters[hasGpx]", "true".to_string()),
                ("pagination[size]", "50".to_string()),
            ]
        );
    }

    #[test]
    fn file_imports_query_maps_filters() {
        let q = list_file_imports_query(&ListFileImportsParams {
            status: Some("failed,skipped".into()),
            page: Some(2),
            ..Default::default()
        });
        assert_eq!(
            q,
            vec![
                ("filters[status]", "failed,skipped".to_string()),
                ("pagination[page]", "2".to_string()),
            ]
        );
    }

    #[test]
    fn maintenance_query_is_optional() {
        assert!(list_maintenance_tasks_query(&ListMaintenanceTasksParams::default()).is_empty());
        let q = list_maintenance_tasks_query(&ListMaintenanceTasksParams {
            gear_id: Some("gear-b1".into()),
        });
        assert_eq!(q, vec![("filters[gearId]", "gear-b1".to_string())]);
    }

    #[test]
    fn update_body_rejects_empty_updates() {
        let err = update_activity_body(&update_params()).unwrap_err();
        assert!(err.contains("at least one"));
    }

    #[test]
    fn update_body_maps_empty_strings_to_null() {
        let mut p = update_params();
        p.description = Some(String::new());
        p.gear_id = Some(String::new());
        let body = update_activity_body(&p).unwrap();
        assert_eq!(body["description"], Value::Null);
        assert_eq!(body["gearId"], Value::Null);
        assert!(body.get("name").is_none());
    }

    #[test]
    fn update_body_includes_set_fields() {
        let mut p = update_params();
        p.name = Some("Evening ride".into());
        p.sport_type = Some("GravelRide".into());
        let body = update_activity_body(&p).unwrap();
        assert_eq!(body["name"], json!("Evening ride"));
        assert_eq!(body["sportType"], json!("GravelRide"));
    }
}
