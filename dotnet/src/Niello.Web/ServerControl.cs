namespace Niello.Web;

/// <summary>A <c>runat="server"</c> element with an <c>ID</c>, i.e. one that gets a designer field.</summary>
/// <param name="TagPrefix">The tag prefix (<c>asp</c>, <c>uc1</c>, ...), or null for HTML server controls such as <c>&lt;form&gt;</c>.</param>
/// <param name="TagName">The tag name without prefix (<c>Button</c>, <c>form</c>, ...).</param>
/// <param name="Id">The value of the <c>ID</c> attribute.</param>
public sealed record ServerControl(string? TagPrefix, string TagName, string Id);
