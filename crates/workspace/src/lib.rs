//! Client-side solution and project model (PLAN.md 4.2, D4).
//!
//! The authoritative project system lives in `eludite-host` (MSBuild evaluation);
//! the shell keeps a light model for Workspace and can classify a
//! `.csproj` as SDK-style or legacy without asking the host.
//!
//! Public API: [`explorer`] turns the host's `eludite/solution/tree` answer into
//! Workspace's tree ([`explorer::SolutionModel`]) with Visual Studio's
//! semantics: folders that mirror the file system, nested files, sorting, and the
//! flattened rows a view draws for a set of expanded nodes; for an opened folder
//! it composes the solution, the Cargo workspace and the folder's files
//! ([`explorer::SolutionModel::compose`], brief 0019). [`cargo`] is the Cargo
//! workspace model read from `cargo metadata` (members, targets, dependencies),
//! and [`folder`] the plain-folder model (the files of a folder, the solution and
//! `Cargo.toml` at its root). [`Solution`] and [`classify_csproj`] are the older
//! project-file helpers.

pub mod cargo;
pub mod explorer;
pub mod folder;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Solution {
    pub path: PathBuf,
    pub projects: Vec<Project>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub path: PathBuf,
    pub kind: ProjectKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProjectKind {
    /// `<Project Sdk="...">` or `<Project><Sdk Name="..."/>`: buildable with `dotnet build`.
    SdkStyle,
    /// Pre-SDK MSBuild 2003 format (WebForms, WCF, most .NET Framework code).
    Legacy,
    Unknown,
}

const MSBUILD_2003_NS: &str = "http://schemas.microsoft.com/developer/msbuild/2003";

/// Classify a `.csproj` (or any MSBuild project) by its root element.
///
/// SDK-style wins if both markers are present. Malformed XML is `Unknown`.
pub fn classify_csproj(xml: &str) -> ProjectKind {
    let xml = xml.strip_prefix('\u{feff}').unwrap_or(xml);
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return ProjectKind::Unknown;
    };
    let root = doc.root_element();
    if root.tag_name().name() != "Project" {
        return ProjectKind::Unknown;
    }
    let has_sdk_attr = root.attribute("Sdk").is_some();
    let has_sdk_element = root
        .children()
        .any(|c| c.is_element() && c.tag_name().name() == "Sdk");
    if has_sdk_attr || has_sdk_element {
        return ProjectKind::SdkStyle;
    }
    let legacy_ns = root.tag_name().namespace() == Some(MSBUILD_2003_NS);
    let has_tools_version = root.attribute("ToolsVersion").is_some();
    if legacy_ns || has_tools_version {
        ProjectKind::Legacy
    } else {
        ProjectKind::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_attribute() {
        let xml = r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup><TargetFramework>net9.0</TargetFramework></PropertyGroup>
</Project>"#;
        assert_eq!(classify_csproj(xml), ProjectKind::SdkStyle);
    }

    #[test]
    fn sdk_element() {
        let xml = r#"<Project>
  <Sdk Name="Microsoft.NET.Sdk.Web" Version="9.0.100" />
  <PropertyGroup><TargetFramework>net9.0</TargetFramework></PropertyGroup>
</Project>"#;
        assert_eq!(classify_csproj(xml), ProjectKind::SdkStyle);
    }

    #[test]
    fn webforms_legacy() {
        let xml = "\u{feff}<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n".to_owned()
            + r#"<Project ToolsVersion="15.0" DefaultTargets="Build" xmlns="http://schemas.microsoft.com/developer/msbuild/2003">
  <Import Project="$(MSBuildExtensionsPath)\$(MSBuildToolsVersion)\Microsoft.Common.props" Condition="Exists('$(MSBuildExtensionsPath)\$(MSBuildToolsVersion)\Microsoft.Common.props')" />
  <PropertyGroup>
    <Configuration Condition=" '$(Configuration)' == '' ">Debug</Configuration>
    <ProjectGuid>{6A1F4B39-8E2C-4D1A-9F3B-2C7E5D8A1B0C}</ProjectGuid>
    <ProjectTypeGuids>{349c5851-65df-11da-9384-00065b846f21};{fae04ec0-301f-11d3-bf4b-00c04f79efbc}</ProjectTypeGuids>
    <OutputType>Library</OutputType>
    <RootNamespace>LegacyShop</RootNamespace>
    <TargetFrameworkVersion>v4.8</TargetFrameworkVersion>
  </PropertyGroup>
  <ItemGroup>
    <Reference Include="System.Web" />
    <Content Include="Default.aspx" />
    <Compile Include="Default.aspx.cs"><DependentUpon>Default.aspx</DependentUpon><SubType>ASPXCodeBehind</SubType></Compile>
    <Compile Include="Default.aspx.designer.cs"><DependentUpon>Default.aspx</DependentUpon></Compile>
  </ItemGroup>
  <Import Project="$(VSToolsPath)\WebApplications\Microsoft.WebApplication.targets" Condition="'$(VSToolsPath)' != ''" />
</Project>"#;
        assert_eq!(classify_csproj(&xml), ProjectKind::Legacy);
    }

    #[test]
    fn legacy_by_namespace_or_tools_version_alone() {
        let ns = r#"<Project xmlns="http://schemas.microsoft.com/developer/msbuild/2003"/>"#;
        let tv = r#"<Project ToolsVersion="4.0"/>"#;
        assert_eq!(classify_csproj(ns), ProjectKind::Legacy);
        assert_eq!(classify_csproj(tv), ProjectKind::Legacy);
    }

    #[test]
    fn unknown() {
        assert_eq!(classify_csproj("<Project/>"), ProjectKind::Unknown);
        assert_eq!(classify_csproj("<Foo Sdk=\"x\"/>"), ProjectKind::Unknown);
        assert_eq!(classify_csproj("not xml"), ProjectKind::Unknown);
    }
}
