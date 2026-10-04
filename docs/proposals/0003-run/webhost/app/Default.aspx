<%@ Page Language="C#" %>
<script runat="server">
    protected void Page_Load(object sender, EventArgs e)
    {
        Greeting.Text = "Hello from System.Web " + typeof(System.Web.UI.Page).Assembly.GetName().Version
            + " on CLR " + Environment.Version + ", OS " + Environment.OSVersion
            + ", name=" + Request.QueryString["name"];
        Items.DataSource = new[] { "WebForms", "ViewState", "DataBinding" };
        Items.DataBind();
    }
</script>
<html><body><form id="f" runat="server">
<h1><asp:Label ID="Greeting" runat="server" /></h1>
<asp:Repeater ID="Items" runat="server"><ItemTemplate><li><%# Container.DataItem %></li></ItemTemplate></asp:Repeater>
<asp:Button ID="Go" runat="server" Text="Post back" />
</form></body></html>
