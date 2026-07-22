use async_trait::async_trait;
use moonlink_core::{MediaProvider, MediaSessionConfig, MediaSessionError, VideoControlMessage};

use crate::cli::AudioSettings;
use crate::external::ExternalSession;
use crate::settings::SettingsStore;
use crate::v4l2_provider::{V4l2Session, V4l2Settings};

#[derive(Debug, Clone)]
pub enum HostMediaProvider {
    Command {
        command: String,
        audio: AudioSettings,
        settings_store: SettingsStore,
    },
    V4l2 {
        settings: V4l2Settings,
        audio: AudioSettings,
        settings_store: SettingsStore,
    },
}

impl HostMediaProvider {
    pub fn command(command: String, audio: AudioSettings, settings_store: SettingsStore) -> Self {
        Self::Command {
            command,
            audio,
            settings_store,
        }
    }

    pub fn v4l2(
        settings: V4l2Settings,
        audio: AudioSettings,
        settings_store: SettingsStore,
    ) -> Self {
        Self::V4l2 {
            settings,
            audio,
            settings_store,
        }
    }
}

pub enum HostMediaSession {
    Command(ExternalSession),
    V4l2(V4l2Session),
}

#[async_trait]
impl MediaProvider for HostMediaProvider {
    type Session = HostMediaSession;

    async fn start_session(
        &self,
        config: MediaSessionConfig,
    ) -> Result<Self::Session, MediaSessionError> {
        println!("starting media session: {config:?}");
        match self {
            Self::Command {
                command,
                audio,
                settings_store,
            } => {
                let current = settings_store.get().await;
                let mut audio = audio.clone();
                current.apply_audio(&mut audio);
                ExternalSession::start(command, current.codec, audio, config)
                    .await
                    .map(HostMediaSession::Command)
            }
            Self::V4l2 {
                settings,
                audio,
                settings_store,
            } => {
                let current = settings_store.get().await;
                let mut settings = settings.clone();
                let mut audio = audio.clone();
                current.apply_v4l2(&mut settings);
                current.apply_audio(&mut audio);
                V4l2Session::start(settings, audio, config)
                    .await
                    .map(HostMediaSession::V4l2)
            }
        }
    }
}

#[async_trait]
impl moonlink_core::MediaSession for HostMediaSession {
    async fn next_video(&mut self) -> Result<Option<moonlink_core::VideoFrame>, MediaSessionError> {
        match self {
            Self::Command(session) => session.next_video().await,
            Self::V4l2(session) => session.next_video().await,
        }
    }

    async fn stop(&mut self) -> Result<(), MediaSessionError> {
        match self {
            Self::Command(session) => session.stop().await,
            Self::V4l2(session) => session.stop().await,
        }
    }

    fn take_audio(&mut self) -> Option<Box<dyn moonlink_core::AudioSession>> {
        match self {
            Self::Command(session) => session.take_audio(),
            Self::V4l2(session) => session.take_audio(),
        }
    }

    async fn handle_video_control(
        &mut self,
        message: VideoControlMessage,
    ) -> Result<(), MediaSessionError> {
        match self {
            Self::Command(session) => session.handle_video_control(message).await,
            Self::V4l2(session) => session.handle_video_control(message).await,
        }
    }
}
