use std::sync::OnceLock;

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Local};
use reqwest::multipart::Form;
use scraper::{selectable::Selectable, ElementRef, Selector};

use crate::reference::Reference;

use super::super::{
    client::{AddFileWithFilename, IliasClient},
    file::File,
    local_file::NamedLocalFile,
    parse_date, IliasElement,
};

#[derive(Debug)]
#[allow(dead_code)]
pub struct Assignment {
    pub name: String,
    pub instructions: Option<String>,
    pub submission_start_date: DateTime<Local>,
    pub submission_end_date: DateTime<Local>,
    pub attachments: Vec<File>,
    submission: Reference<AssignmentSubmission>,
}

static INFO_SCREEN_SELECTOR: OnceLock<Selector> = OnceLock::new();
static INFO_SCREEN_NAME_SELECTOR: OnceLock<Selector> = OnceLock::new();

static NAME_SELECTOR: OnceLock<Selector> = OnceLock::new();
static INFO_PROPERTY_VALUE_SELECTOR: OnceLock<Selector> = OnceLock::new();
static INFO_PROPERTY_KEY_SELECTOR: OnceLock<Selector> = OnceLock::new();
static PROPERTY_ROW_SELECTOR: OnceLock<Selector> = OnceLock::new();
static SUBMISSION_PAGE_SELECTOR: OnceLock<Selector> = OnceLock::new();

impl IliasElement for Assignment {
    fn type_identifier() -> Option<&'static str> {
        Some("ass")
    }

    fn querypath_from_id(id: &str) -> Option<String> {
        Some(format!(
            "goto.php?target={}_{}&client_id=produktiv",
            Self::type_identifier().unwrap(),
            id
        ))
    }

    fn parse(element: ElementRef, _ilias_client: &IliasClient) -> Result<Self> {
        let name_selector = NAME_SELECTOR.get_or_init(|| {
            Selector::parse(".ilAssignmentHeader").expect("Could not parse selector")
        });

        let info_screen_selector = INFO_SCREEN_SELECTOR
            .get_or_init(|| Selector::parse(".ilInfoScreenSec").expect("Could not parse selector"));
        let info_screen_name_selector = INFO_SCREEN_NAME_SELECTOR
            .get_or_init(|| Selector::parse(".ilHeader").expect("Could not parse selector"));
        let info_property_value_selector = INFO_PROPERTY_VALUE_SELECTOR.get_or_init(|| {
            Selector::parse(".il_InfoScreenPropertyValue").expect("Could not parse selector")
        });
        let property_row_selector = PROPERTY_ROW_SELECTOR
            .get_or_init(|| Selector::parse(".form-group").expect("Could not parse selector"));
        let submission_page_selector = SUBMISSION_PAGE_SELECTOR
            .get_or_init(|| Selector::parse("a").expect("Could not parse selector"));

        let name = element
            .select(name_selector)
            .next()
            .context("Did not find name")?
            .text()
            .collect();

        let info_screens: Vec<_> = element
            .select(info_screen_selector)
            .map(|info_screen| {
                (
                    info_screen,
                    info_screen
                        .select(info_screen_name_selector)
                        .next()
                        .context(anyhow!("Could not find name of info screen"))
                        .unwrap()
                        .text()
                        .collect::<String>(),
                )
            })
            .collect();

        let instruction_info = info_screens.iter().find_map(|(screen, name)| {
            if ["Arbeitsanweisung", "Work Instructions"].contains(&name.as_str()) {
                Some(screen)
            } else {
                None
            }
        });
        let instructions = instruction_info.and_then(|instruction_info| {
            Some(
                instruction_info
                    .select(info_property_value_selector)
                    .next()?
                    .text()
                    .collect(),
            )
        });

        let schedule_info = info_screens
            .iter()
            .find_map(|(screen, name)| {
                if ["Schedule", "Terminplan"].contains(&name.as_str()) {
                    Some(*screen)
                } else {
                    None
                }
            })
            .context("Did not find schedule")?;
        let submission_start_date =
            Self::get_value_for_keys(schedule_info, &["Startzeit", "Start Time"])?;
        let submission_start_date = parse_date(submission_start_date.trim())?;
        let submission_end_date =
            Self::get_value_for_keys(schedule_info, &["Abgabetermin", "Edit Until"])?;
        let submission_end_date = parse_date(submission_end_date.trim())?;

        let attachment_info = info_screens.iter().find_map(|(screen, name)| {
            if ["Dateien", "Files"].contains(&name.as_str()) {
                Some(screen)
            } else {
                None
            }
        });
        let attachments = attachment_info.map_or(vec![], |attachment_info| {
            let file_rows = attachment_info.select(property_row_selector);
            file_rows
                .map(|file_row| {
                    let mut children = file_row.child_elements();
                    let filename = children
                        .next()
                        .expect("Did not find filename")
                        .text()
                        .collect();
                    let download_link = children
                        .next()
                        .expect("Did not find download button")
                        .child_elements()
                        .next()
                        .expect("Did not find download link")
                        .attr("href")
                        .expect("Did not find download href");

                    File {
                        name: filename,
                        description: "".to_string(),
                        download_querypath: Some(download_link.to_string()),
                        date: None,
                        id: None,
                    }
                })
                .collect()
        });

        let submission_info = info_screens.iter().find_map(|(screen, name)| {
            if ["Ihre Einreichung", "Your Submission"].contains(&name.as_str()) {
                Some(*screen)
            } else {
                None
            }
        });
        let submission_page_querypath = submission_info
            .and_then(|info| {
                Self::get_value_element_for_keys(info, &["Abgegebene Dateien", "Submitted Files"])
                    .ok()
            })
            .and_then(|info| info.select(submission_page_selector).next())
            .map(|link| link.attr("href").expect("Could not find href in link"))
            .map(|querypath| querypath.to_string());

        Ok(Assignment {
            name,
            instructions,
            submission_start_date,
            submission_end_date,
            attachments,
            submission: Reference::from_optional_querypath(submission_page_querypath),
        })
    }
}

impl Assignment {
    pub fn is_active(&self) -> bool {
        self.submission_end_date >= Local::now() && self.submission_start_date <= Local::now()
    }

    pub fn get_submission(&mut self, ilias_client: &IliasClient) -> Option<&AssignmentSubmission> {
        let submission = &mut self.submission;
        match submission {
            Reference::Unavailable => None,
            Reference::Resolved(ref submission) => Some(submission),
            Reference::Unresolved(querypath) => {
                let ass_sub = AssignmentSubmission::parse_submissions_page(
                    ilias_client
                        .get_querypath(querypath)
                        .expect("Could not get submission page")
                        .root_element(),
                    ilias_client,
                )
                .expect("Could not parse submission page");
                *submission = Reference::Resolved(ass_sub);

                submission.try_get_resolved()
            }
        }
    }

    fn get_value_element_for_keys<'a>(
        info_screen: ElementRef<'a>,
        keys: &[&str],
    ) -> Result<ElementRef<'a>> {
        let property_row_selector = PROPERTY_ROW_SELECTOR
            .get_or_init(|| Selector::parse(".form-group").expect("Could not parse selector"));
        let info_property_value_selector = INFO_PROPERTY_VALUE_SELECTOR.get_or_init(|| {
            Selector::parse(".il_InfoScreenPropertyValue").expect("Could not parse selector")
        });
        let info_property_key_selector = INFO_PROPERTY_KEY_SELECTOR.get_or_init(|| {
            Selector::parse(".il_InfoScreenProperty").expect("Could not parse selector")
        });

        info_screen
            .select(property_row_selector)
            .find(|&element| {
                keys.contains(
                    &element
                        .select(info_property_key_selector)
                        .next()
                        .expect("Property without key")
                        .text()
                        .collect::<String>()
                        .as_str(),
                )
            })
            .context(anyhow!("Did not find {:?} property", keys))?
            .select(info_property_value_selector)
            .next()
            .context(anyhow!("Did not find value for {:?} property", keys))
    }

    fn get_value_for_keys(info_screen: ElementRef, keys: &[&str]) -> Result<String> {
        Ok(Self::get_value_element_for_keys(info_screen, keys)?
            .text()
            .collect())
    }
}

#[derive(Debug)]
pub struct AssignmentSubmission {
    pub submissions: Vec<File>,
    delete_querypath: String,
    upload_querypath: String,
}

static UPLOAD_BUTTON_SELECTOR: OnceLock<Selector> = OnceLock::new();
static CONTENT_FORM_SELECTOR: OnceLock<Selector> = OnceLock::new();
static FILE_ROW_SELECTOR: OnceLock<Selector> = OnceLock::new();

impl AssignmentSubmission {
    fn parse_submissions_page(
        submission_page: ElementRef,
        ilias_client: &IliasClient,
    ) -> Result<AssignmentSubmission> {
        let upload_button_selector = UPLOAD_BUTTON_SELECTOR.get_or_init(|| {
            Selector::parse(r#"nav div.navbar-header button"#).expect("Could not parse selector")
        });
        let content_form_selector = CONTENT_FORM_SELECTOR.get_or_init(|| {
            Selector::parse(r#"div#ilContentContainer form"#).expect("Could not parse selector")
        });
        let file_row_selector = FILE_ROW_SELECTOR
            .get_or_init(|| Selector::parse(r#"form tbody tr"#).expect("Could not parse selector"));

        let file_rows = submission_page.select(file_row_selector);
        let uploaded_files = file_rows
            .filter(|&row| row.child_elements().count() > 1)
            .map(|file_row| {
                let mut children = file_row.child_elements();

                let id = children
                    .next()
                    .expect("Did not find first column in table")
                    .child_elements()
                    .next()
                    .expect("Did not find checkbox")
                    .attr("value")
                    .expect("Did not find id");
                let file_name = children
                    .next()
                    .expect("Did not find second column")
                    .text()
                    .collect();
                let submission_date = loop {
                    let parsed_date = parse_date(
                        &children
                            .next()
                            .expect("Did not find date column")
                            .text()
                            .collect::<String>(),
                    );
                    match parsed_date {
                        Ok(date) => break date,
                        _ => continue,
                    }
                };
                let download_querypath = children
                    .last()
                    .expect("Did not find last column")
                    .child_elements()
                    .next()
                    .expect("Did not find download link")
                    .attr("href")
                    .expect("Did not find href attribute");

                File {
                    id: Some(id.to_string()),
                    name: file_name,
                    description: "".to_string(),
                    date: Some(submission_date),
                    download_querypath: Some(download_querypath.to_string()),
                }
            })
            .collect();

        let delete_querypath = submission_page
            .select(content_form_selector)
            .next()
            .context("Did not find deltion form")?
            .value()
            .attr("action")
            .context("Did not find action attribute")?
            .to_string();

        let upload_form_querypath = submission_page
            .select(upload_button_selector)
            .next()
            .context("Did not find upload button")?
            .attr("data-action")
            .context("Did not find data-action on upload button")?;
        let upload_page = ilias_client.get_querypath(upload_form_querypath)?;
        let upload_querypath = upload_page
            .select(content_form_selector)
            .next()
            .context("Did not find upload form")?
            .value()
            .attr("action")
            .context("Did not find action attribute")?
            .to_string();

        Ok(AssignmentSubmission {
            submissions: uploaded_files,
            delete_querypath,
            upload_querypath,
        })
    }

    pub fn delete_files(&self, ilias_client: &IliasClient, files: &[&File]) -> Result<()> {
        let mut form_args = files
            .iter()
            .map(|&file| file.id.clone().expect("Files to delete must have an id"))
            .map(|id| ("delivered[]", id))
            .collect::<Vec<_>>();
        form_args.push(("cmd[deleteDelivered]", String::from("Löschen")));

        ilias_client.post_querypath_form(&self.delete_querypath, &form_args)?;
        Ok(())
    }

    pub fn upload_files(&self, ilias_client: &IliasClient, files: &[NamedLocalFile]) -> Result<()> {
        let mut form = Form::new();

        for (index, file_data) in files.iter().enumerate() {
            form = form
                .file_with_name(
                    format!("deliver[{}]", index),
                    ilias_client.construct_file_part(&file_data.path),
                    file_data.name.clone(),
                )?
                .text("cmd[uploadFile]", "Hochladen")
                .text("ilfilehash", "aaaa");
        }

        ilias_client.post_querypath_multipart(&self.upload_querypath, form)?;
        Ok(())
        // TODO: Maybe push files to submission here
    }
}
