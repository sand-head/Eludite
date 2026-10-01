namespace Eludite.Web;

/// <summary>The main directive kind of a WebForms markup file.</summary>
public enum DirectiveKind
{
    /// <summary><c>&lt;%@ Page %&gt;</c> in an <c>.aspx</c> file.</summary>
    Page,

    /// <summary><c>&lt;%@ Control %&gt;</c> in an <c>.ascx</c> user control.</summary>
    Control,

    /// <summary><c>&lt;%@ Master %&gt;</c> in a <c>.master</c> page.</summary>
    Master,
}

/// <summary>The parsed main directive of an <c>.aspx</c>, <c>.ascx</c> or <c>.master</c> file.</summary>
/// <param name="Kind">Which directive it is.</param>
/// <param name="Language">The <c>Language</c> attribute (e.g. <c>C#</c>, <c>VB</c>).</param>
/// <param name="CodeBehind">The <c>CodeBehind</c> file (Web Application Projects).</param>
/// <param name="CodeFile">The <c>CodeFile</c> file (Web Site Projects).</param>
/// <param name="Inherits">The code-behind class the generated page derives from.</param>
/// <param name="AutoEventWireup">The <c>AutoEventWireup</c> flag, or null when absent or not a boolean.</param>
/// <param name="OtherAttributes">Every remaining attribute, keyed case-insensitively.</param>
public sealed record AspxDirective(
    DirectiveKind Kind,
    string? Language,
    string? CodeBehind,
    string? CodeFile,
    string? Inherits,
    bool? AutoEventWireup,
    IReadOnlyDictionary<string, string> OtherAttributes);
